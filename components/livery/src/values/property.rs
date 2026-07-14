use std::{fmt, str::FromStr};

use super::{Length, LengthPercentage, ParseError, format_number, keyword_value};

keyword_value! {
    /// CSS border line style.
    pub enum BorderStyle {
        None => "none",
        Hidden => "hidden",
        Dotted => "dotted",
        Dashed => "dashed",
        Solid => "solid",
        Double => "double",
        Groove => "groove",
        Ridge => "ridge",
        Inset => "inset",
        Outset => "outset",
    }
}

keyword_value! {
    /// Display keywords required by the Cambium lane and baseline UA sheet.
    pub enum Display {
        None => "none",
        Inline => "inline",
        Block => "block",
        InlineBlock => "inline-block",
        Table => "table",
        TableRowGroup => "table-row-group",
        TableRow => "table-row",
        TableCell => "table-cell",
        TableCaption => "table-caption",
    }
}

keyword_value! {
    pub enum FontStyle {
        Normal => "normal",
        Italic => "italic",
        Oblique => "oblique",
    }
}

keyword_value! {
    pub enum ListStyleType {
        None => "none",
        Disc => "disc",
        Decimal => "decimal",
    }
}

keyword_value! {
    pub enum Overflow {
        Visible => "visible",
        Hidden => "hidden",
        Clip => "clip",
        Scroll => "scroll",
        Auto => "auto",
    }
}

keyword_value! {
    pub enum Position {
        Static => "static",
        Relative => "relative",
        Absolute => "absolute",
        Sticky => "sticky",
        Fixed => "fixed",
    }
}

keyword_value! {
    pub enum TextWrapMode {
        Wrap => "wrap",
        Nowrap => "nowrap",
    }
}

keyword_value! {
    pub enum WhiteSpaceCollapse {
        Collapse => "collapse",
        Discard => "discard",
        Preserve => "preserve",
        PreserveBreaks => "preserve-breaks",
        PreserveSpaces => "preserve-spaces",
        BreakSpaces => "break-spaces",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BorderWidth {
    Thin,
    Medium,
    Thick,
    Length(Length),
}

impl FromStr for BorderWidth {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "thin" => Ok(Self::Thin),
            "medium" => Ok(Self::Medium),
            "thick" => Ok(Self::Thick),
            _ => input.parse::<Length>().map(Self::Length),
        }
    }
}

impl fmt::Display for BorderWidth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thin => formatter.write_str("thin"),
            Self::Medium => formatter.write_str("medium"),
            Self::Thick => formatter.write_str("thick"),
            Self::Length(length) => length.fmt(formatter),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFamily {
    UserAgentDefault,
    SystemUi,
    Named(Box<str>),
}

impl FromStr for FontFamily {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("system-ui") {
            return Ok(Self::SystemUi);
        }
        if input.eq_ignore_ascii_case("depends-on-user-agent") {
            return Ok(Self::UserAgentDefault);
        }
        if input.is_empty() || input.contains(',') {
            return Err(ParseError::expected("one seed font family"));
        }
        let unquoted = input
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(input);
        Ok(Self::Named(unquoted.into()))
    }
}

impl fmt::Display for FontFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserAgentDefault => formatter.write_str("depends-on-user-agent"),
            Self::SystemUi => formatter.write_str("system-ui"),
            Self::Named(name) if name.contains(char::is_whitespace) => {
                write!(formatter, "\"{name}\"")
            },
            Self::Named(name) => formatter.write_str(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontSize {
    Medium,
    Value(LengthPercentage),
}

impl FromStr for FontSize {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.trim().eq_ignore_ascii_case("medium") {
            Ok(Self::Medium)
        } else {
            input.parse::<LengthPercentage>().map(Self::Value)
        }
    }
}

impl fmt::Display for FontSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Medium => formatter.write_str("medium"),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontWeight {
    Normal,
    Bold,
    Bolder,
    Lighter,
    Number(u16),
}

impl FromStr for FontWeight {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "bold" => Ok(Self::Bold),
            "bolder" => Ok(Self::Bolder),
            "lighter" => Ok(Self::Lighter),
            number => number
                .parse::<u16>()
                .ok()
                .filter(|number| (1..=1000).contains(number))
                .map(Self::Number)
                .ok_or_else(|| ParseError::expected("a font weight from 1 through 1000")),
        }
    }
}

impl fmt::Display for FontWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => formatter.write_str("normal"),
            Self::Bold => formatter.write_str("bold"),
            Self::Bolder => formatter.write_str("bolder"),
            Self::Lighter => formatter.write_str("lighter"),
            Self::Number(number) => number.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Auto,
    MinContent,
    MaxContent,
    FitContent(LengthPercentage),
    Value(LengthPercentage),
}

impl FromStr for Size {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if input.eq_ignore_ascii_case("min-content") {
            return Ok(Self::MinContent);
        }
        if input.eq_ignore_ascii_case("max-content") {
            return Ok(Self::MaxContent);
        }
        if input.len() > 13
            && input[..12].eq_ignore_ascii_case("fit-content(")
            && input.ends_with(')')
        {
            return input[12..input.len() - 1]
                .parse::<LengthPercentage>()
                .map(Self::FitContent);
        }
        input.parse::<LengthPercentage>().map(Self::Value)
    }
}

impl fmt::Display for Size {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::MinContent => formatter.write_str("min-content"),
            Self::MaxContent => formatter.write_str("max-content"),
            Self::FitContent(value) => write!(formatter, "fit-content({value})"),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

macro_rules! auto_length_percentage {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub enum $name {
            Auto,
            Value(LengthPercentage),
        }

        impl FromStr for $name {
            type Err = ParseError;

            fn from_str(input: &str) -> Result<Self, Self::Err> {
                if input.trim().eq_ignore_ascii_case("auto") {
                    Ok(Self::Auto)
                } else {
                    input.parse::<LengthPercentage>().map(Self::Value)
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::Auto => formatter.write_str("auto"),
                    Self::Value(value) => value.fmt(formatter),
                }
            }
        }
    };
}

auto_length_percentage!(Inset);
auto_length_percentage!(Margin);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Value(LengthPercentage),
}

impl FromStr for LineHeight {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("normal") {
            return Ok(Self::Normal);
        }
        if let Ok(number) = input.parse::<f32>()
            && number.is_finite()
            && number >= 0.0
        {
            return Ok(Self::Number(number));
        }
        input.parse::<LengthPercentage>().map(Self::Value)
    }
}

impl fmt::Display for LineHeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => formatter.write_str("normal"),
            Self::Number(number) => formatter.write_str(&format_number(*number)),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opacity(f32);

impl Opacity {
    pub const ONE: Self = Self(1.0);

    pub const fn value(self) -> f32 {
        self.0
    }
}

impl FromStr for Opacity {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let value = if let Some(percentage) = input.strip_suffix('%') {
            percentage.trim().parse::<f32>().map(|value| value / 100.0)
        } else {
            input.parse::<f32>()
        }
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ParseError::expected("a finite opacity number or percentage"))?;
        Ok(Self(value.clamp(0.0, 1.0)))
    }
}

impl fmt::Display for Opacity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_number(self.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Padding(pub LengthPercentage);

impl Padding {
    pub const ZERO: Self = Self(LengthPercentage::ZERO);
}

impl FromStr for Padding {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.parse::<LengthPercentage>()?;
        let negative = match value {
            LengthPercentage::Zero => false,
            LengthPercentage::Length(length) => length.value < 0.0,
            LengthPercentage::Percentage(value) => value < 0.0,
            LengthPercentage::Calc(_) => false,
        };
        if negative {
            return Err(ParseError::expected("a non-negative padding"));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Padding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextDecorationLine(u8);

impl TextDecorationLine {
    pub const NONE: Self = Self(0);
    const UNDERLINE: u8 = 1 << 0;
    const OVERLINE: u8 = 1 << 1;
    const LINE_THROUGH: u8 = 1 << 2;
    const BLINK: u8 = 1 << 3;

    pub const fn contains_underline(self) -> bool {
        self.0 & Self::UNDERLINE != 0
    }
}

impl FromStr for TextDecorationLine {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::NONE);
        }
        let mut flags = 0;
        for keyword in input.split_ascii_whitespace() {
            let flag = match keyword.to_ascii_lowercase().as_str() {
                "underline" => Self::UNDERLINE,
                "overline" => Self::OVERLINE,
                "line-through" => Self::LINE_THROUGH,
                "blink" => Self::BLINK,
                _ => return Err(ParseError::expected("text-decoration-line keywords")),
            };
            if flags & flag != 0 {
                return Err(ParseError::expected("unique text-decoration-line keywords"));
            }
            flags |= flag;
        }
        if flags == 0 {
            return Err(ParseError::expected("text-decoration-line keywords"));
        }
        Ok(Self(flags))
    }
}

impl fmt::Display for TextDecorationLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::NONE {
            return formatter.write_str("none");
        }
        let mut first = true;
        for (flag, name) in [
            (Self::UNDERLINE, "underline"),
            (Self::OVERLINE, "overline"),
            (Self::LINE_THROUGH, "line-through"),
            (Self::BLINK, "blink"),
        ] {
            if self.0 & flag == 0 {
                continue;
            }
            if !first {
                formatter.write_str(" ")?;
            }
            formatter.write_str(name)?;
            first = false;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZIndex {
    Auto,
    Integer(i32),
}

impl FromStr for ZIndex {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.trim().eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            input
                .trim()
                .parse::<i32>()
                .map(Self::Integer)
                .map_err(|_| ParseError::expected("auto or an integer"))
        }
    }
}

impl fmt::Display for ZIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Integer(value) => value.fmt(formatter),
        }
    }
}
