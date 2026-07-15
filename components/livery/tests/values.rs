use std::fmt::Debug;

use livery::values::{
    Alignment, AnimationName, AspectRatio, BackgroundImage, BackgroundPosition, BackgroundRepeat,
    BorderStyle, BorderWidth, BoxShadow, BoxSizing, Color, CssValue, Display, Duration,
    FlexDirection, FlexFactor, FlexWrap, FontFamily, FontSize, FontStyle, FontWeight, Gap, Inset,
    LengthPercentage, LineHeight, ListStyleType, Margin, Opacity, Order, Overflow, Padding,
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
    assert_round_trip::<Alignment>("space-between");
    assert_round_trip::<FlexDirection>("column");
    assert_round_trip::<FlexFactor>("1.5");
    assert_round_trip::<FlexWrap>("wrap");
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
fn invalid_seed_values_are_rejected() {
    assert!("florp".parse::<Display>().is_err());
    assert!("-1rem".parse::<Padding>().is_err());
    assert!("1100".parse::<FontWeight>().is_err());
    assert!("calc(100% 1px)".parse::<LengthPercentage>().is_err());
    assert!("rgb(300, 0, 0)".parse::<Color>().is_err());
    assert!("NaN".parse::<Opacity>().is_err());
    assert!("perspective(20px)".parse::<Transform>().is_err());
    assert_eq!("120%".parse::<Opacity>().unwrap().value(), 1.0);
    assert_eq!("-0.5".parse::<Opacity>().unwrap().value(), 0.0);
}
