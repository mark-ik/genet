//! Typed values used by Livery's first property lane.

use std::{error::Error, fmt, str::FromStr};

mod color;
mod length;
mod property;

pub use color::Color;
pub use length::{CalcLengthPercentage, Length, LengthPercentage, LengthUnit};
pub use property::{
    Alignment, AnimationName, AspectRatio, BackgroundImage, BackgroundPosition, BackgroundRepeat,
    BorderStyle, BorderWidth, BoxShadow, BoxShadowValue, BoxSizing, Display, Duration,
    FlexDirection, FlexFactor, FlexWrap, Float, FontFamily, FontSize, FontStyle, FontWeight, Gap,
    GridAutoFlow, GridPlacement, GridTemplate, GridTrack, Inset, LineHeight, ListStyleType, Margin,
    Opacity, Order, Overflow, Padding, PointerEvents, Position, Radius, Size, Spacing, TextAlign,
    TextDecorationColor, TextDecorationLine, TextWrapMode, TimingFunction, Transform,
    TransformFunction, TransitionProperty, VerticalAlign, Visibility, WhiteSpaceCollapse, ZIndex,
};

/// A rejected CSS value from Livery's bounded first-lane grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseError {
    expected: &'static str,
}

impl ParseError {
    pub(crate) const fn expected(expected: &'static str) -> Self {
        Self { expected }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "expected {}", self.expected)
    }
}

impl Error for ParseError {}

/// Common parse/serialize contract for Livery value types.
pub trait CssValue: Sized + fmt::Display + FromStr<Err = ParseError> {
    fn parse_css(input: &str) -> Result<Self, ParseError> {
        input.parse()
    }

    fn to_css_string(&self) -> String {
        self.to_string()
    }
}

impl<T> CssValue for T where T: Sized + fmt::Display + FromStr<Err = ParseError> {}

pub(crate) fn format_number(value: f32) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    value.to_string()
}

macro_rules! keyword_value {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $css:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum $name {
            $($variant),+
        }

        impl std::str::FromStr for $name {
            type Err = super::ParseError;

            fn from_str(input: &str) -> Result<Self, Self::Err> {
                match input.trim().to_ascii_lowercase().as_str() {
                    $($css => Ok(Self::$variant),)+
                    _ => Err(super::ParseError::expected(stringify!($name))),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(match self {
                    $(Self::$variant => $css,)+
                })
            }
        }
    };
}

pub(crate) use keyword_value;
