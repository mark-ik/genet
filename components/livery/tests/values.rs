use std::fmt::Debug;

use livery::media::{ViewportSize, ViewportSizes};
use livery::values::{
    Alignment, AnimationName, AspectRatio, BackgroundImage, BackgroundPosition, BackgroundRepeat,
    BorderStyle, BorderWidth, BoxShadow, BoxSizing, Color, CssValue, Display, Duration,
    FlexDirection, FlexFactor, FlexWrap, Float, FontFamily, FontSize, FontStyle, FontWeight, Gap,
    Inset, LengthPercentage, LengthUnit, LineHeight, ListStyleType, Margin, Opacity, Order,
    Overflow, Padding, PointerEvents, Position, Radius, RelativeLengthEnvironment, Rotate, Scale,
    Size, Spacing, TextAlign, TextDecorationLine, TextWrapMode, TimingFunction, Transform,
    TransitionProperty, VerticalAlign, Visibility, WhiteSpaceCollapse, ZIndex,
};
use livery::{canonicalize_specified_longhand, canonicalize_specified_value};

fn assert_round_trip<T>(css: &str)
where
    T: CssValue + Debug + PartialEq,
{
    let parsed = T::parse_css(css).unwrap_or_else(|error| panic!("{css}: {error}"));
    let serialized = parsed.to_css_string();
    let reparsed = T::parse_css(&serialized)
        .unwrap_or_else(|error| panic!("{css} serialized as {serialized}: {error}"));
    assert_eq!(parsed, reparsed, "{css} serialized as {serialized}");
}

#[test]
fn specified_border_canonicalizes_nested_calc_width() {
    assert_eq!(
        canonicalize_specified_value("border", "calc(calc(10px)) solid pink").as_deref(),
        Some("calc(10px) solid pink")
    );
    assert_eq!(
        canonicalize_specified_value("border", "solid calc(2 * 5px) pink").as_deref(),
        Some("solid calc(10px) pink")
    );
    assert_eq!(
        canonicalize_specified_value("border", "calc(10%) solid pink"),
        None
    );
}

#[test]
fn length_percentage_and_calc_values_round_trip() {
    for value in [
        "0",
        "12px",
        "-2em",
        "1.5rem",
        "37.5%",
        "calc(100% - 16px + 2em - 0.5rem)",
        "calc(33.333332% + 0.1234567px)",
    ] {
        assert_round_trip::<LengthPercentage>(value);
    }
}

#[test]
fn nested_calc_reduces_with_dimensional_arithmetic() {
    for (source, expected) in [
        ("calc(20px + calc(80px))", "calc(100px)"),
        ("calc(calc(100px))", "calc(100px)"),
        ("calc(calc(2) * calc(50px))", "calc(100px)"),
        ("calc(calc(150px*2/3))", "calc(100px)"),
        ("calc(calc(2 * calc(calc(3)) + 4) * 10px)", "calc(100px)"),
        ("calc(50px + calc(40%))", "calc(40% + 50px)"),
        ("calc(10px + 1em)", "calc(1em + 10px)"),
    ] {
        let parsed = source
            .parse::<LengthPercentage>()
            .unwrap_or_else(|error| panic!("{source}: {error}"));
        assert_eq!(parsed.to_string(), expected, "{source}");
        assert_eq!(
            canonicalize_specified_longhand("left", source).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn calc_rejects_dimensionally_invalid_or_malformed_math() {
    for source in [
        "calc(2 + 10px)",
        "calc(10px * 2px)",
        "calc(10px / 0)",
        "calc(10px + 2)",
        "calc(100px+20px)",
    ] {
        assert!(
            source.parse::<LengthPercentage>().is_err(),
            "accepted {source}"
        );
    }
}

#[test]
fn color_values_round_trip() {
    for value in [
        "transparent",
        "currentcolor",
        "CanvasText",
        "#abc",
        "#202733",
        "#33669980",
        "rgb(32, 39, 51)",
        "rgb(10 20 30 / 50%)",
        "rebeccapurple",
    ] {
        assert_round_trip::<Color>(value);
    }
}

#[test]
fn catalog_property_values_round_trip() {
    assert_round_trip::<Display>("inline-block");
    assert_round_trip::<AspectRatio>("16 / 9");
    assert_round_trip::<BoxSizing>("border-box");
    assert_round_trip::<BoxShadow>("0 2px 4px #00000055");
    assert_round_trip::<BackgroundImage>("linear-gradient(red, blue)");
    assert_round_trip::<BackgroundImage>("url(data:image/png;base64,seed)");
    assert_round_trip::<BackgroundPosition>("center 10px");
    assert_round_trip::<BackgroundRepeat>("no-repeat");
    assert_round_trip::<Duration>("100ms");
    assert_round_trip::<AnimationName>("fade");
    assert_round_trip::<TimingFunction>("ease-in-out");
    assert_round_trip::<TransitionProperty>("opacity");
    assert_round_trip::<TransitionProperty>("background-color");
    assert_round_trip::<TransitionProperty>("color");
    assert_round_trip::<TransitionProperty>("border-top-color");
    assert_round_trip::<TransitionProperty>("border-bottom-color");
    assert_round_trip::<TransitionProperty>("border-left-color");
    assert_round_trip::<TransitionProperty>("border-right-color");
    assert_round_trip::<TransitionProperty>("border-top-width");
    assert_round_trip::<TransitionProperty>("border-bottom-width");
    assert_round_trip::<TransitionProperty>("border-left-width");
    assert_round_trip::<TransitionProperty>("border-right-width");
    assert_round_trip::<TransitionProperty>("border-radius");
    assert_round_trip::<TransitionProperty>("transform");
    assert_round_trip::<TransitionProperty>("background-position");
    assert_round_trip::<TransitionProperty>("box-shadow");
    assert_round_trip::<TransitionProperty>("background-image");
    assert_round_trip::<TransitionProperty>("border-top-style");
    assert_round_trip::<TransitionProperty>("border-bottom-style");
    assert_round_trip::<TransitionProperty>("border-left-style");
    assert_round_trip::<TransitionProperty>("border-right-style");
    assert_round_trip::<TransitionProperty>("border-top-style, border-right-style");
    assert_round_trip::<TransitionProperty>("background-repeat");
    assert_round_trip::<TransitionProperty>("opacity, background-color");
    assert_round_trip::<TransitionProperty>("color, opacity, background-color");
    assert_round_trip::<TransitionProperty>("opacity, border-left-color, border-right-color");
    assert_round_trip::<Alignment>("space-between");
    assert_round_trip::<FlexDirection>("column");
    assert_round_trip::<FlexFactor>("1.5");
    assert_round_trip::<FlexWrap>("wrap");
    assert_round_trip::<Float>("left");
    assert_round_trip::<Gap>("12px");
    assert_round_trip::<FontFamily>("system-ui");
    assert_round_trip::<FontFamily>("\"Atkinson Hyperlegible\"");
    assert_round_trip::<FontSize>("1.5rem");
    assert_round_trip::<FontStyle>("italic");
    assert_round_trip::<FontWeight>("700");
    assert_round_trip::<Size>("42rem");
    assert_round_trip::<Size>("fit-content(80%)");
    assert_round_trip::<Size>("none");
    assert_round_trip::<Inset>("25%");
    assert_round_trip::<LineHeight>("1.5");
    assert_round_trip::<ListStyleType>("decimal");
    assert_round_trip::<Margin>("auto");
    assert_round_trip::<Margin>("0.5rem");
    assert_round_trip::<Opacity>("50%");
    assert_round_trip::<Transform>("translate(12px, 4px) scale(1.5) rotate(30deg)");
    assert_round_trip::<Transform>("matrix(1, 2, 3, 4, 5, 6)");
    assert_round_trip::<Overflow>("hidden");
    assert_round_trip::<Padding>("0.75rem");
    assert_round_trip::<PointerEvents>("none");
    assert_round_trip::<Position>("absolute");
    assert_round_trip::<Order>("-1");
    assert_round_trip::<Radius>("12px");
    assert_round_trip::<Rotate>("30deg");
    assert_round_trip::<Scale>("1.5");
    assert_round_trip::<Spacing>("0.1em");
    assert_round_trip::<TextAlign>("center");
    assert_round_trip::<BorderStyle>("solid");
    assert_round_trip::<BorderWidth>("1px");
    assert_round_trip::<TextDecorationLine>("underline overline");
    assert_round_trip::<TextWrapMode>("nowrap");
    assert_round_trip::<Visibility>("hidden");
    assert_round_trip::<VerticalAlign>("text-top");
    assert_round_trip::<VerticalAlign>("-2px");
    assert_round_trip::<WhiteSpaceCollapse>("preserve");
    assert_round_trip::<ZIndex>("10");
}

#[test]
fn radius_interpolation_preserves_the_bounded_length_family() {
    let from = "0".parse::<Radius>().expect("zero radius");
    let to = "20px".parse::<Radius>().expect("px radius");
    assert_eq!(from.interpolate(to, 0.5).to_string(), "10px");
}

#[test]
fn border_width_interpolation_preserves_computed_px_values() {
    let from = "thin".parse::<BorderWidth>().expect("thin width");
    let to = "5px".parse::<BorderWidth>().expect("px width");
    assert_eq!(from.interpolate(to, 0.5).to_string(), "3px");

    let from = "2px".parse::<BorderWidth>().expect("from width");
    let to = "10px".parse::<BorderWidth>().expect("to width");
    assert_eq!(from.interpolate(to, 0.5).to_string(), "6px");

    let from = "1em".parse::<BorderWidth>().expect("from em width");
    let to = "10px".parse::<BorderWidth>().expect("to px width");
    assert_eq!(from.interpolate(to, 0.25), from);
    assert_eq!(from.interpolate(to, 0.75), to);
}

#[test]
fn transform_interpolation_preserves_matching_function_shape() {
    let from = "translate(0px, 0px)".parse::<Transform>().expect("from");
    let to = "translate(20px, 4px)".parse::<Transform>().expect("to");
    assert_eq!(
        from.interpolate(&to, 0.5).to_string(),
        "translate(10px, 2px)"
    );
}

#[test]
fn transform_matrices_cover_skew_and_mismatched_list_interpolation() {
    let skew = "skewX(45deg)".parse::<Transform>().expect("skew");
    let skew = skew.to_matrix(16.0, (0.0, 0.0)).expect("skew matrix");
    assert!((skew.a - 1.0).abs() < 0.0001);
    assert!(skew.b.abs() < 0.0001);
    assert!((skew.c - 1.0).abs() < 0.0001);
    assert!((skew.d - 1.0).abs() < 0.0001);

    let from = "translate(20px, 4px)"
        .parse::<Transform>()
        .expect("from transform");
    let to = "scale(2)".parse::<Transform>().expect("to transform");
    let middle = from
        .interpolate(&to, 0.5)
        .to_matrix(16.0, (0.0, 0.0))
        .expect("interpolated matrix");
    assert!((middle.a - 1.5).abs() < 0.0001);
    assert!((middle.d - 1.5).abs() < 0.0001);
    assert!((middle.e - 10.0).abs() < 0.0001);
    assert!((middle.f - 2.0).abs() < 0.0001);
}

#[test]
fn transform_percentages_resolve_against_the_reference_box() {
    let transform = "translate(25%, 50%)"
        .parse::<Transform>()
        .expect("percentage transform");
    let matrix = transform
        .to_matrix(16.0, (100.0, 50.0))
        .expect("percentage matrix");
    assert!((matrix.e - 25.0).abs() < 0.0001);
    assert!((matrix.f - 25.0).abs() < 0.0001);

    let value = "calc(25% + 2em)"
        .parse::<LengthPercentage>()
        .expect("mixed calc")
        .resolve_font_relative(10.0, 16.0);
    assert_eq!(value.to_string(), "calc(25% + 20px)");
    assert!((value.to_px(10.0, 16.0, 100.0) - 45.0).abs() < 0.0001);
}

#[test]
fn background_position_interpolation_preserves_each_component() {
    let from = "left top"
        .parse::<BackgroundPosition>()
        .expect("from position");
    let to = "right bottom"
        .parse::<BackgroundPosition>()
        .expect("to position");
    assert_eq!(from.interpolate(to, 0.5).to_string(), "50% 50%");
}

#[test]
fn background_image_interpolation_preserves_gradient_stops() {
    let from = "linear-gradient(red, blue)"
        .parse::<BackgroundImage>()
        .expect("from image");
    let to = "linear-gradient(white, black)"
        .parse::<BackgroundImage>()
        .expect("to image");
    assert_eq!(
        from.interpolate(&to, 0.5).to_string(),
        "linear-gradient(#ff8080, #000080)"
    );
}

#[test]
fn border_style_interpolation_switches_at_the_midpoint() {
    let from = "solid".parse::<BorderStyle>().expect("from style");
    let to = "dashed".parse::<BorderStyle>().expect("to style");
    assert_eq!(from.interpolate(to, 0.49), from);
    assert_eq!(from.interpolate(to, 0.5), to);
}

#[test]
fn background_repeat_interpolation_switches_at_the_midpoint() {
    let from = "no-repeat"
        .parse::<BackgroundRepeat>()
        .expect("from repeat mode");
    let to = "repeat"
        .parse::<BackgroundRepeat>()
        .expect("to repeat mode");
    assert_eq!(from.interpolate(to, 0.49), from);
    assert_eq!(from.interpolate(to, 0.5), to);
}

#[test]
fn box_shadow_interpolation_preserves_matching_shape() {
    let from = "0 0 0 red".parse::<BoxShadow>().expect("from shadow");
    let to = "20px 4px 10px blue"
        .parse::<BoxShadow>()
        .expect("to shadow");
    assert_eq!(
        from.interpolate(&to, 0.5).to_string(),
        "10px 2px 5px 0 #800080"
    );
}

#[test]
fn viewport_units_serialize_and_resolve_from_the_device_size() {
    for (source, expected) in [
        ("10vw", 80.0),
        ("10vh", 60.0),
        ("10vmin", 60.0),
        ("10vmax", 80.0),
    ] {
        let value = source.parse::<LengthPercentage>().expect(source);
        assert_eq!(value.to_string(), source);
        let resolved = value.resolve_viewport(800.0, 600.0);
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }

    let mixed = "calc(10% + 10px + 1vmin)"
        .parse::<LengthPercentage>()
        .expect("mixed viewport calc")
        .resolve_viewport(800.0, 600.0);
    assert_eq!(mixed.to_string(), "calc(10% + 16px)");
    assert!((mixed.to_px(16.0, 16.0, 200.0) - 36.0).abs() < 0.001);
}

#[test]
fn viewport_tiers_and_logical_axes_resolve_from_distinct_device_metrics() {
    let viewport = ViewportSizes {
        small: ViewportSize::new(300.0, 200.0),
        large: ViewportSize::new(600.0, 400.0),
        dynamic: ViewportSize::new(450.0, 250.0),
    };
    let environment = RelativeLengthEnvironment::viewport(viewport);
    for (source, expected) in [
        ("1vw", 6.0),
        ("1vh", 4.0),
        ("1vi", 6.0),
        ("1vb", 4.0),
        ("1vmin", 4.0),
        ("1vmax", 6.0),
        ("1svw", 3.0),
        ("1svh", 2.0),
        ("1svi", 3.0),
        ("1svb", 2.0),
        ("1svmin", 2.0),
        ("1svmax", 3.0),
        ("1lvw", 6.0),
        ("1lvh", 4.0),
        ("1lvi", 6.0),
        ("1lvb", 4.0),
        ("1lvmin", 4.0),
        ("1lvmax", 6.0),
        ("1dvw", 4.5),
        ("1dvh", 2.5),
        ("1dvi", 4.5),
        ("1dvb", 2.5),
        ("1dvmin", 2.5),
        ("1dvmax", 4.5),
    ] {
        let resolved = source
            .parse::<LengthPercentage>()
            .expect(source)
            .resolve_relative(environment);
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }

    let calc = "calc(1svw + 1lvh + 1dvi)"
        .parse::<LengthPercentage>()
        .expect("tiered viewport calc")
        .resolve_relative(environment);
    assert_eq!(calc.to_string(), "calc(11.5px)");

    let vertical = environment.with_vertical_writing(true);
    for (source, expected) in [
        ("1vi", 4.0),
        ("1vb", 6.0),
        ("1svi", 2.0),
        ("1svb", 3.0),
        ("1dvi", 2.5),
        ("1dvb", 4.5),
    ] {
        let resolved = source
            .parse::<LengthPercentage>()
            .expect(source)
            .resolve_relative(vertical);
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }
}

#[test]
fn container_units_resolve_each_axis_or_fall_back_to_the_small_viewport() {
    let viewport = ViewportSizes {
        small: ViewportSize::new(200.0, 80.0),
        large: ViewportSize::new(400.0, 160.0),
        dynamic: ViewportSize::new(300.0, 120.0),
    };
    let contained = RelativeLengthEnvironment::containers(viewport, Some(300.0), Some(400.0));
    for (source, expected) in [
        ("10cqw", 30.0),
        ("10cqi", 30.0),
        ("10cqh", 40.0),
        ("10cqb", 40.0),
        ("10cqmin", 30.0),
        ("10cqmax", 40.0),
    ] {
        let resolved = source
            .parse::<LengthPercentage>()
            .expect(source)
            .resolve_relative(contained);
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }

    let fallback = "calc(10cqi + 10cqb)"
        .parse::<LengthPercentage>()
        .expect("container fallback calc")
        .resolve_relative(RelativeLengthEnvironment::container_fallback(viewport));
    assert_eq!(fallback.to_string(), "calc(28px)");

    let vertical = RelativeLengthEnvironment::container_axes(
        viewport,
        Some(500.0),
        Some(300.0),
        Some(300.0),
        Some(500.0),
        true,
    );
    for (source, expected) in [
        ("10cqw", 50.0),
        ("10cqh", 30.0),
        ("10cqi", 30.0),
        ("10cqb", 50.0),
    ] {
        let resolved = source
            .parse::<LengthPercentage>()
            .expect(source)
            .resolve_relative(vertical);
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }
}

#[test]
fn comparison_math_resolves_after_its_environmental_bases_are_known() {
    let viewport = ViewportSizes {
        small: ViewportSize::new(300.0, 200.0),
        large: ViewportSize::new(600.0, 400.0),
        dynamic: ViewportSize::new(450.0, 250.0),
    };
    let environment = RelativeLengthEnvironment::containers(viewport, Some(300.0), Some(400.0));
    for (source, expected) in [
        ("min(1lvw, 1lvh)", 4.0),
        ("max(1svw, 1svh)", 3.0),
        ("max(10cqi, 10cqb)", 40.0),
        ("clamp(10px, 35px, 30px)", 30.0),
        ("clamp(10px /* lower */, 35px, 30px)", 30.0),
        ("clamp(30px, 100px, 20px)", 30.0),
    ] {
        let resolved = source
            .parse::<LengthPercentage>()
            .expect(source)
            .resolve_relative(environment)
            .resolve_font_relative(16.0, 16.0);
        assert_eq!(resolved.to_string(), format!("{expected}px"));
        assert!((resolved.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }

    let percentage = "clamp(10px, 50%, 80px)"
        .parse::<LengthPercentage>()
        .expect("comparison with a percentage")
        .resolve_relative(environment)
        .resolve_font_relative(16.0, 16.0);
    assert!((percentage.to_px(16.0, 16.0, 100.0) - 50.0).abs() < 0.001);

    for (source, expected) in [
        ("min(1px, max(2px, 3px))", 1.0),
        ("calc(0px + clamp(10px, 20px, 30px))", 20.0),
        ("calc(0px - clamp(10px, 20px, 30px))", -20.0),
        ("clamp(none, 30px, 33px)", 30.0),
        ("clamp(30px, 33px, none)", 33.0),
        ("clamp(1600px / 1em * 1px, 1em / 1rem * 1px, none)", 80.0),
    ] {
        let value = source.parse::<LengthPercentage>().expect(source);
        assert!((value.to_px(20.0, 16.0, 0.0) - expected).abs() < 0.001);
    }
}

#[test]
fn stepped_math_preserves_dimensions_and_sign_rules() {
    for (source, expected) in [
        ("round(10px, 6px)", 12.0),
        ("round(up, 101px, 10px)", 110.0),
        ("round(down, 106px, 10px)", 100.0),
        ("round(to-zero, -105px, 10px)", -100.0),
        ("mod(-18px, 5px)", 2.0),
        ("mod(18px, -5px)", -2.0),
        ("rem(-18px, 5px)", -3.0),
        ("rem(18px, -5px)", 3.0),
    ] {
        let value = source.parse::<LengthPercentage>().expect(source);
        assert!((value.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.001);
    }

    let mixed = "mod(18px, 100% / 15)"
        .parse::<LengthPercentage>()
        .expect("percentage step");
    assert!((mixed.to_px(16.0, 16.0, 225.0) - 3.0).abs() < 0.001);
}

#[test]
fn trigonometric_math_accepts_numbers_and_canonical_angles() {
    for (source, expected) in [
        ("calc(100px * sin(30deg + 1.0471976rad))", 100.0),
        ("calc(20px * cos(0))", 20.0),
        ("calc(10px * tan(0.125turn))", 10.0),
        ("calc(10px * sin(asin(1)))", 10.0),
        ("calc(10px * cos(acos(1)))", 10.0),
        ("calc(10px * tan(atan(1)))", 10.0),
        (
            "calc(10px * sin(atan2(1px, -1px)))",
            std::f32::consts::FRAC_1_SQRT_2 * 10.0,
        ),
    ] {
        let value = source.parse::<LengthPercentage>().expect(source);
        assert!((value.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.01);
    }
}

#[test]
fn exponential_math_composes_inside_length_expressions() {
    for (source, expected) in [
        ("calc(100px * pow(2, pow(2, 2)))", 1600.0),
        ("calc(100px * sqrt(100))", 1000.0),
        ("hypot(3px, 4px)", 5.0),
        ("calc(100px * hypot(3, 4))", 500.0),
        ("calc(10px * exp(log(2)))", 20.0),
        ("calc(10px * log(8, 2))", 30.0),
    ] {
        let value = source.parse::<LengthPercentage>().expect(source);
        assert!((value.to_px(16.0, 16.0, 0.0) - expected).abs() < 0.01);
    }
}

#[test]
fn number_and_angle_math_feed_individual_transform_properties() {
    for (source, expected) in [
        ("sin(30deg)", 0.5),
        ("cos(0)", 1.0),
        ("tan(45deg)", 1.0),
        ("pow(2, 3)", 8.0),
        ("sqrt(81)", 9.0),
        ("hypot(3, 4)", 5.0),
        ("log(8, 2)", 3.0),
        ("exp(0)", 1.0),
    ] {
        let scale = source.parse::<Scale>().expect(source);
        assert!((scale.factor().expect("scale factor") - expected).abs() < 0.001);
    }

    for (source, expected) in [
        ("asin(1)", std::f32::consts::FRAC_PI_2),
        ("acos(0)", std::f32::consts::FRAC_PI_2),
        ("atan(1)", std::f32::consts::FRAC_PI_4),
        ("atan2(1, -1)", 3.0 * std::f32::consts::FRAC_PI_4),
    ] {
        let rotate = source.parse::<Rotate>().expect(source);
        assert!((rotate.radians().expect("rotation") - expected).abs() < 0.001);
    }

    assert_eq!("pow(2, 3)".parse::<ZIndex>(), Ok(ZIndex::Integer(8)));
}

#[test]
fn calc_serialization_orders_viewport_terms_canonically() {
    for (source, expected) in [
        ("calc(10px + 1vmin + 10%)", "calc(10% + 10px + 1vmin)"),
        ("calc(10px + 1vmin)", "calc(10px + 1vmin)"),
        ("calc(10px + 1em)", "calc(1em + 10px)"),
        ("calc(1vmin - 10px)", "calc(-10px + 1vmin)"),
        ("calc(-10px + 1em)", "calc(1em - 10px)"),
        ("calc(-10px)", "calc(-10px)"),
    ] {
        assert_eq!(
            source
                .parse::<LengthPercentage>()
                .expect(source)
                .to_string(),
            expected
        );
    }

    let eight_relative = "calc(1cqb + 1cqh + 1cqi + 1cqmax + 1cqmin + 1cqw + 1dvb + 1dvh)";
    assert_eq!(
        eight_relative
            .parse::<LengthPercentage>()
            .expect("eight distinct relative terms")
            .to_string(),
        eight_relative
    );
    assert!(
        "calc(1cqb + 1cqh + 1cqi + 1cqmax + 1cqmin + 1cqw + 1dvb + 1dvh + \
         1dvi + 1dvmax + 1dvmin + 1dvw + 1lvb + 1lvh + 1lvi + 1lvmax + 1lvmin)"
            .parse::<LengthPercentage>()
            .is_err()
    );
}

#[test]
fn invalid_seed_values_are_rejected() {
    assert!("florp".parse::<Display>().is_err());
    assert!("-1rem".parse::<Padding>().is_err());
    assert!("1100".parse::<FontWeight>().is_err());
    assert!("calc(100% 1px)".parse::<LengthPercentage>().is_err());
    assert!("rgb(300, 0, 0)".parse::<Color>().is_err());
    assert!("all, color".parse::<TransitionProperty>().is_err());
    assert!("opacity, opacity".parse::<TransitionProperty>().is_err());
    assert!("NaN".parse::<Opacity>().is_err());
    assert!("perspective(20px)".parse::<Transform>().is_err());
    assert_eq!("120%".parse::<Opacity>().unwrap().value(), 1.0);
    assert_eq!("-0.5".parse::<Opacity>().unwrap().value(), 0.0);
}

#[test]
fn absolute_css_length_units_round_trip_and_resolve() {
    let cases = [
        ("1in", LengthUnit::In, 96.0),
        ("2.54cm", LengthUnit::Cm, 96.0),
        ("25.4mm", LengthUnit::Mm, 96.0),
        ("101.6q", LengthUnit::Q, 96.0),
        ("72pt", LengthUnit::Pt, 96.0),
        ("6pc", LengthUnit::Pc, 96.0),
    ];
    for (source, unit, expected_px) in cases {
        let LengthPercentage::Length(length) = source
            .parse::<LengthPercentage>()
            .expect("absolute CSS length")
        else {
            panic!("expected a length value for {source}");
        };
        assert_eq!(length.unit, unit);
        assert!((unit.to_px(length.value, 16.0, 16.0) - expected_px).abs() < 0.001);
        assert_eq!(length.to_string(), source);
    }
}
