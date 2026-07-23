use std::{fmt, str::FromStr};

use super::{ParseError, format_number};

/// Length units needed by the audited Cambium and UA-sheet corpus.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LengthUnit {
    Px,
    Em,
    Rem,
    In,
    Cm,
    Mm,
    Q,
    Pt,
    Pc,
}

impl LengthUnit {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Px => "px",
            Self::Em => "em",
            Self::Rem => "rem",
            Self::In => "in",
            Self::Cm => "cm",
            Self::Mm => "mm",
            Self::Q => "q",
            Self::Pt => "pt",
            Self::Pc => "pc",
        }
    }

    /// Resolve an absolute or font-relative unit against the current CSS
    /// font size (`em`) and root font size (`rem`). CSS absolute units use the
    /// 96dpi reference pixel defined by CSS Values.
    pub const fn to_px(self, value: f32, em: f32, rem: f32) -> f32 {
        value
            * match self {
                Self::Px => 1.0,
                Self::Em => em,
                Self::Rem => rem,
                Self::In => 96.0,
                Self::Cm => 96.0 / 2.54,
                Self::Mm => 96.0 / 25.4,
                Self::Q => 96.0 / 101.6,
                Self::Pt => 96.0 / 72.0,
                Self::Pc => 16.0,
            }
    }
}

/// A finite CSS length in one of Livery's seed units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    pub const ZERO: Self = Self {
        value: 0.0,
        unit: LengthUnit::Px,
    };

    pub const fn px(value: f32) -> Self {
        Self {
            value,
            unit: LengthUnit::Px,
        }
    }

    pub const fn em(value: f32) -> Self {
        Self {
            value,
            unit: LengthUnit::Em,
        }
    }

    pub const fn rem(value: f32) -> Self {
        Self {
            value,
            unit: LengthUnit::Rem,
        }
    }
}

impl FromStr for Length {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input == "0" || input == "+0" || input == "-0" {
            return Ok(Self::ZERO);
        }
        let lower = input.to_ascii_lowercase();
        for (suffix, unit) in [
            ("rem", LengthUnit::Rem),
            ("px", LengthUnit::Px),
            ("em", LengthUnit::Em),
            ("in", LengthUnit::In),
            ("cm", LengthUnit::Cm),
            ("mm", LengthUnit::Mm),
            ("pt", LengthUnit::Pt),
            ("pc", LengthUnit::Pc),
            ("q", LengthUnit::Q),
        ] {
            if let Some(number) = lower.strip_suffix(suffix) {
                let value = number
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| ParseError::expected("a finite CSS length"))?;
                if value.is_finite() {
                    return Ok(Self { value, unit });
                }
            }
        }
        Err(ParseError::expected("a CSS length"))
    }
}

impl fmt::Display for Length {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.value == 0.0 {
            return formatter.write_str("0");
        }
        write!(
            formatter,
            "{}{}",
            format_number(self.value),
            self.unit.suffix()
        )
    }
}

/// A reduced linear `calc()` length-percentage expression.
///
/// Parsing and dimensional arithmetic live in the harvested calc module. This
/// compact result is the form Livery needs for serialization, interpolation,
/// and later used-value resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalcLengthPercentage {
    pub percentage: f32,
    pub px: f32,
    pub em: f32,
    pub rem: f32,
}

impl CalcLengthPercentage {
    fn terms(self) -> [(f32, &'static str); 4] {
        [
            (self.percentage * 100.0, "%"),
            (self.em, "em"),
            (self.px, "px"),
            (self.rem, "rem"),
        ]
    }
}

impl fmt::Display for CalcLengthPercentage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calc(")?;
        let mut wrote = false;
        for (value, unit) in self.terms() {
            if value == 0.0 {
                continue;
            }
            if wrote {
                formatter.write_str(if value.is_sign_negative() {
                    " - "
                } else {
                    " + "
                })?;
            } else if value.is_sign_negative() {
                formatter.write_str("-")?;
            }
            write!(formatter, "{}{}", format_number(value.abs()), unit)?;
            wrote = true;
        }
        if !wrote {
            formatter.write_str("0px")?;
        }
        formatter.write_str(")")
    }
}

/// A length, percentage, or linear `calc()` combination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LengthPercentage {
    Zero,
    Length(Length),
    /// Stored as a unit value: `1.0` is `100%`.
    Percentage(f32),
    Calc(CalcLengthPercentage),
}

impl LengthPercentage {
    pub const ZERO: Self = Self::Zero;

    /// Whether resolving this value needs a percentage basis supplied by the
    /// consuming property.
    pub const fn has_percentage(self) -> bool {
        match self {
            Self::Percentage(_) => true,
            Self::Calc(calc) => calc.percentage != 0.0,
            Self::Zero | Self::Length(_) => false,
        }
    }

    /// Resolve absolute and font-relative terms while preserving any
    /// percentage for the property's later used-value basis.
    pub fn resolve_font_relative(self, em: f32, rem: f32) -> Self {
        match self {
            Self::Zero | Self::Percentage(_) => self,
            Self::Length(length) => {
                Self::Length(Length::px(length.unit.to_px(length.value, em, rem)))
            },
            Self::Calc(calc) => Self::Calc(CalcLengthPercentage {
                percentage: calc.percentage,
                px: calc.px + calc.em * em + calc.rem * rem,
                em: 0.0,
                rem: 0.0,
            }),
        }
    }

    /// Resolve the value against the percentage basis defined by its
    /// consuming property.
    pub fn to_px(self, em: f32, rem: f32, percentage_basis: f32) -> f32 {
        match self {
            Self::Zero => 0.0,
            Self::Length(length) => length.unit.to_px(length.value, em, rem),
            Self::Percentage(value) => value * percentage_basis,
            Self::Calc(calc) => {
                calc.percentage * percentage_basis + calc.px + calc.em * em + calc.rem * rem
            },
        }
    }

    /// Interpolate the bounded scalar forms shared by paint and geometry
    /// properties. Zero adopts the other endpoint's unit; mixed non-zero
    /// units and calc expressions remain discrete until their value ratchet.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        match (self, other) {
            (Self::Zero, Self::Zero) => Self::ZERO,
            (Self::Length(from), Self::Length(to)) if from.unit == to.unit => {
                Self::Length(Length {
                    value: from.value + (to.value - from.value) * progress,
                    unit: from.unit,
                })
            },
            (Self::Percentage(from), Self::Percentage(to)) => {
                Self::Percentage(from + (to - from) * progress)
            },
            (Self::Zero, Self::Length(to)) | (Self::Length(to), Self::Zero) => {
                let unit = to.unit;
                let target = to.value;
                let (from, to) = if matches!(self, Self::Zero) {
                    (0.0, target)
                } else {
                    (target, 0.0)
                };
                Self::Length(Length {
                    value: from + (to - from) * progress,
                    unit,
                })
            },
            (Self::Zero, Self::Percentage(to)) | (Self::Percentage(to), Self::Zero) => {
                let (from, to) = if matches!(self, Self::Zero) {
                    (0.0, to)
                } else {
                    (to, 0.0)
                };
                Self::Percentage(from + (to - from) * progress)
            },
            _ => {
                if progress < 0.5 {
                    self
                } else {
                    other
                }
            },
        }
    }
}

fn parse_atomic(input: &str) -> Result<LengthPercentage, ParseError> {
    if input == "0" || input == "+0" || input == "-0" {
        return Ok(LengthPercentage::Zero);
    }
    if let Some(number) = input.strip_suffix('%') {
        let value = number
            .parse::<f32>()
            .map_err(|_| ParseError::expected("a percentage"))?;
        if value.is_finite() {
            return Ok(LengthPercentage::Percentage(value / 100.0));
        }
    }
    input.parse::<Length>().map(LengthPercentage::Length)
}

impl FromStr for LengthPercentage {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("calc("))
        {
            return super::calc::parse_length_percentage(input).map(Self::Calc);
        }
        parse_atomic(input)
    }
}

impl fmt::Display for LengthPercentage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("0"),
            Self::Length(length) => length.fmt(formatter),
            Self::Percentage(value) => write!(formatter, "{}%", format_number(value * 100.0)),
            Self::Calc(calc) => calc.fmt(formatter),
        }
    }
}
