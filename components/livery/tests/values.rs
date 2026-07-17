use std::fmt::Debug;

use livery::values::{
    Alignment, AnimationName, AspectRatio, BackgroundImage, BackgroundPosition, BackgroundRepeat,
    BorderStyle, BorderWidth, BoxShadow, BoxSizing, Color, CssValue, Display, Duration,
    FlexDirection, FlexFactor, FlexWrap, Float, FontFamily, FontSize, FontStyle, FontWeight, Gap,
    Inset, LengthPercentage, LineHeight, ListStyleType, Margin, Opacity, Order, Overflow, Padding,
    PointerEvents, Position, Radius, Size, Spacing, TextAlign, TextDecorationLine, TextWrapMode,
    TimingFunction, Transform, TransitionProperty, Visibility, WhiteSpaceCollapse, ZIndex,
};

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
    assert_round_trip::<TransitionProperty>("border-radius");
    assert_round_trip::<TransitionProperty>("transform");
    assert_round_trip::<TransitionProperty>("background-position");
    assert_round_trip::<TransitionProperty>("box-shadow");
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
    assert_round_trip::<Overflow>("hidden");
    assert_round_trip::<Padding>("0.75rem");
    assert_round_trip::<PointerEvents>("none");
    assert_round_trip::<Position>("absolute");
    assert_round_trip::<Order>("-1");
    assert_round_trip::<Radius>("12px");
    assert_round_trip::<Spacing>("0.1em");
    assert_round_trip::<TextAlign>("center");
    assert_round_trip::<BorderStyle>("solid");
    assert_round_trip::<BorderWidth>("1px");
    assert_round_trip::<TextDecorationLine>("underline overline");
    assert_round_trip::<TextWrapMode>("nowrap");
    assert_round_trip::<Visibility>("hidden");
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
fn transform_interpolation_preserves_matching_function_shape() {
    let from = "translate(0px, 0px)".parse::<Transform>().expect("from");
    let to = "translate(20px, 4px)".parse::<Transform>().expect("to");
    assert_eq!(
        from.interpolate(&to, 0.5).to_string(),
        "translate(10px, 2px)"
    );
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
