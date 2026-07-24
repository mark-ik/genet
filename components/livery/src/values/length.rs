use std::{fmt, str::FromStr};

use crate::media::ViewportSizes;

use super::{ParseError, format_number};

const RELATIVE_UNIT_COUNT: usize = 30;
const MAX_CALC_RELATIVE_TERMS: usize = 8;
const RELATIVE_INDEX_BITS: usize = 5;
const RELATIVE_LEN_SHIFT: usize = MAX_CALC_RELATIVE_TERMS * RELATIVE_INDEX_BITS;
const RELATIVE_INDEX_MASK: u64 = (1 << RELATIVE_INDEX_BITS) - 1;

/// Length units needed by the audited Cambium and UA-sheet corpus.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
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
    Vw,
    Vh,
    Vi,
    Vb,
    Vmin,
    Vmax,
    Svw,
    Svh,
    Svi,
    Svb,
    Svmin,
    Svmax,
    Lvw,
    Lvh,
    Lvi,
    Lvb,
    Lvmin,
    Lvmax,
    Dvw,
    Dvh,
    Dvi,
    Dvb,
    Dvmin,
    Dvmax,
    Cqw,
    Cqh,
    Cqi,
    Cqb,
    Cqmin,
    Cqmax,
}

const RELATIVE_UNITS: [LengthUnit; RELATIVE_UNIT_COUNT] = [
    LengthUnit::Cqb,
    LengthUnit::Cqh,
    LengthUnit::Cqi,
    LengthUnit::Cqmax,
    LengthUnit::Cqmin,
    LengthUnit::Cqw,
    LengthUnit::Dvb,
    LengthUnit::Dvh,
    LengthUnit::Dvi,
    LengthUnit::Dvmax,
    LengthUnit::Dvmin,
    LengthUnit::Dvw,
    LengthUnit::Lvb,
    LengthUnit::Lvh,
    LengthUnit::Lvi,
    LengthUnit::Lvmax,
    LengthUnit::Lvmin,
    LengthUnit::Lvw,
    LengthUnit::Svb,
    LengthUnit::Svh,
    LengthUnit::Svi,
    LengthUnit::Svmax,
    LengthUnit::Svmin,
    LengthUnit::Svw,
    LengthUnit::Vb,
    LengthUnit::Vh,
    LengthUnit::Vi,
    LengthUnit::Vmax,
    LengthUnit::Vmin,
    LengthUnit::Vw,
];

/// Whether a container-relative axis should remain deferred, use the small
/// viewport fallback, or resolve from an eligible query container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContainerAxisSize {
    Deferred,
    Fallback,
    Size(f32),
}

/// All environment inputs needed to resolve relative lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelativeLengthEnvironment {
    pub viewport: ViewportSizes,
    pub container_width: ContainerAxisSize,
    pub container_height: ContainerAxisSize,
    pub container_inline: ContainerAxisSize,
    pub container_block: ContainerAxisSize,
    pub vertical_writing: bool,
}

impl RelativeLengthEnvironment {
    /// Resolve viewport units but retain container units for a later layout
    /// pass.
    pub const fn viewport(viewport: ViewportSizes) -> Self {
        Self {
            viewport,
            container_width: ContainerAxisSize::Deferred,
            container_height: ContainerAxisSize::Deferred,
            container_inline: ContainerAxisSize::Deferred,
            container_block: ContainerAxisSize::Deferred,
            vertical_writing: false,
        }
    }

    /// Resolve missing query-container axes against the small viewport.
    pub const fn container_fallback(viewport: ViewportSizes) -> Self {
        Self {
            viewport,
            container_width: ContainerAxisSize::Fallback,
            container_height: ContainerAxisSize::Fallback,
            container_inline: ContainerAxisSize::Fallback,
            container_block: ContainerAxisSize::Fallback,
            vertical_writing: false,
        }
    }

    /// Resolve each container axis independently. A missing eligible axis
    /// falls back to the corresponding small viewport axis.
    pub fn containers(
        viewport: ViewportSizes,
        inline_size: Option<f32>,
        block_size: Option<f32>,
    ) -> Self {
        Self {
            viewport,
            container_width: inline_size
                .map_or(ContainerAxisSize::Fallback, ContainerAxisSize::Size),
            container_height: block_size
                .map_or(ContainerAxisSize::Fallback, ContainerAxisSize::Size),
            container_inline: inline_size
                .map_or(ContainerAxisSize::Fallback, ContainerAxisSize::Size),
            container_block: block_size
                .map_or(ContainerAxisSize::Fallback, ContainerAxisSize::Size),
            vertical_writing: false,
        }
    }

    /// Supply independently selected physical and logical query-container
    /// axes. This is needed when vertical writing makes the nearest eligible
    /// width and inline-size containers differ.
    pub fn container_axes(
        viewport: ViewportSizes,
        width: Option<f32>,
        height: Option<f32>,
        inline: Option<f32>,
        block: Option<f32>,
        vertical_writing: bool,
    ) -> Self {
        let axis =
            |size: Option<f32>| size.map_or(ContainerAxisSize::Fallback, ContainerAxisSize::Size);
        Self {
            viewport,
            container_width: axis(width),
            container_height: axis(height),
            container_inline: axis(inline),
            container_block: axis(block),
            vertical_writing,
        }
    }

    pub const fn with_vertical_writing(mut self, vertical_writing: bool) -> Self {
        self.vertical_writing = vertical_writing;
        self
    }

    pub const fn uniform_viewport(width: f32, height: f32) -> Self {
        Self::viewport(ViewportSizes::uniform(width, height))
    }
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
            Self::Vw => "vw",
            Self::Vh => "vh",
            Self::Vi => "vi",
            Self::Vb => "vb",
            Self::Vmin => "vmin",
            Self::Vmax => "vmax",
            Self::Svw => "svw",
            Self::Svh => "svh",
            Self::Svi => "svi",
            Self::Svb => "svb",
            Self::Svmin => "svmin",
            Self::Svmax => "svmax",
            Self::Lvw => "lvw",
            Self::Lvh => "lvh",
            Self::Lvi => "lvi",
            Self::Lvb => "lvb",
            Self::Lvmin => "lvmin",
            Self::Lvmax => "lvmax",
            Self::Dvw => "dvw",
            Self::Dvh => "dvh",
            Self::Dvi => "dvi",
            Self::Dvb => "dvb",
            Self::Dvmin => "dvmin",
            Self::Dvmax => "dvmax",
            Self::Cqw => "cqw",
            Self::Cqh => "cqh",
            Self::Cqi => "cqi",
            Self::Cqb => "cqb",
            Self::Cqmin => "cqmin",
            Self::Cqmax => "cqmax",
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
                unit if unit.is_relative() => {
                    panic!("environment-relative length must be resolved before px consumption")
                },
                _ => panic!("unknown CSS length unit"),
            }
    }

    pub const fn is_viewport_relative(self) -> bool {
        matches!(
            self,
            Self::Vw
                | Self::Vh
                | Self::Vi
                | Self::Vb
                | Self::Vmin
                | Self::Vmax
                | Self::Svw
                | Self::Svh
                | Self::Svi
                | Self::Svb
                | Self::Svmin
                | Self::Svmax
                | Self::Lvw
                | Self::Lvh
                | Self::Lvi
                | Self::Lvb
                | Self::Lvmin
                | Self::Lvmax
                | Self::Dvw
                | Self::Dvh
                | Self::Dvi
                | Self::Dvb
                | Self::Dvmin
                | Self::Dvmax
        )
    }

    pub const fn is_container_relative(self) -> bool {
        matches!(
            self,
            Self::Cqw | Self::Cqh | Self::Cqi | Self::Cqb | Self::Cqmin | Self::Cqmax
        )
    }

    pub const fn is_relative(self) -> bool {
        self.is_viewport_relative() || self.is_container_relative()
    }

    const fn relative_index(self) -> Option<usize> {
        let mut index = 0;
        while index < RELATIVE_UNIT_COUNT {
            if RELATIVE_UNITS[index] as u8 == self as u8 {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    fn relative_basis(self, environment: RelativeLengthEnvironment) -> Option<f32> {
        let viewport = environment.viewport;
        match self {
            Self::Vw | Self::Lvw => Some(viewport.large.width),
            Self::Vh | Self::Lvh => Some(viewport.large.height),
            Self::Vi | Self::Lvi => Some(viewport_inline(viewport.large, environment)),
            Self::Vb | Self::Lvb => Some(viewport_block(viewport.large, environment)),
            Self::Vmin | Self::Lvmin => Some(viewport.large.width.min(viewport.large.height)),
            Self::Vmax | Self::Lvmax => Some(viewport.large.width.max(viewport.large.height)),
            Self::Svw => Some(viewport.small.width),
            Self::Svh => Some(viewport.small.height),
            Self::Svi => Some(viewport_inline(viewport.small, environment)),
            Self::Svb => Some(viewport_block(viewport.small, environment)),
            Self::Svmin => Some(viewport.small.width.min(viewport.small.height)),
            Self::Svmax => Some(viewport.small.width.max(viewport.small.height)),
            Self::Dvw => Some(viewport.dynamic.width),
            Self::Dvh => Some(viewport.dynamic.height),
            Self::Dvi => Some(viewport_inline(viewport.dynamic, environment)),
            Self::Dvb => Some(viewport_block(viewport.dynamic, environment)),
            Self::Dvmin => Some(viewport.dynamic.width.min(viewport.dynamic.height)),
            Self::Dvmax => Some(viewport.dynamic.width.max(viewport.dynamic.height)),
            Self::Cqw => container_axis(environment.container_width, viewport.small.width),
            Self::Cqh => container_axis(environment.container_height, viewport.small.height),
            Self::Cqi => container_axis(
                environment.container_inline,
                viewport_inline(viewport.small, environment),
            ),
            Self::Cqb => container_axis(
                environment.container_block,
                viewport_block(viewport.small, environment),
            ),
            Self::Cqmin => {
                let inline = container_axis(
                    environment.container_inline,
                    viewport_inline(viewport.small, environment),
                )?;
                let block = container_axis(
                    environment.container_block,
                    viewport_block(viewport.small, environment),
                )?;
                Some(inline.min(block))
            },
            Self::Cqmax => {
                let inline = container_axis(
                    environment.container_inline,
                    viewport_inline(viewport.small, environment),
                )?;
                let block = container_axis(
                    environment.container_block,
                    viewport_block(viewport.small, environment),
                )?;
                Some(inline.max(block))
            },
            _ => None,
        }
    }
}

fn viewport_inline(
    viewport: crate::media::ViewportSize,
    environment: RelativeLengthEnvironment,
) -> f32 {
    if environment.vertical_writing {
        viewport.height
    } else {
        viewport.width
    }
}

fn viewport_block(
    viewport: crate::media::ViewportSize,
    environment: RelativeLengthEnvironment,
) -> f32 {
    if environment.vertical_writing {
        viewport.width
    } else {
        viewport.height
    }
}

fn container_axis(axis: ContainerAxisSize, fallback: f32) -> Option<f32> {
    match axis {
        ContainerAxisSize::Deferred => None,
        ContainerAxisSize::Fallback => Some(fallback),
        ContainerAxisSize::Size(value) => Some(value),
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

    pub fn resolve_viewport(self, viewport_width: f32, viewport_height: f32) -> Self {
        self.resolve_relative(RelativeLengthEnvironment::uniform_viewport(
            viewport_width,
            viewport_height,
        ))
    }

    pub fn resolve_relative(self, environment: RelativeLengthEnvironment) -> Self {
        self.unit
            .relative_basis(environment)
            .map_or(self, |basis| Self::px(self.value * basis / 100.0))
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
            ("cqmin", LengthUnit::Cqmin),
            ("cqmax", LengthUnit::Cqmax),
            ("dvmin", LengthUnit::Dvmin),
            ("dvmax", LengthUnit::Dvmax),
            ("lvmin", LengthUnit::Lvmin),
            ("lvmax", LengthUnit::Lvmax),
            ("svmin", LengthUnit::Svmin),
            ("svmax", LengthUnit::Svmax),
            ("vmin", LengthUnit::Vmin),
            ("vmax", LengthUnit::Vmax),
            ("cqw", LengthUnit::Cqw),
            ("cqh", LengthUnit::Cqh),
            ("cqi", LengthUnit::Cqi),
            ("cqb", LengthUnit::Cqb),
            ("dvw", LengthUnit::Dvw),
            ("dvh", LengthUnit::Dvh),
            ("dvi", LengthUnit::Dvi),
            ("dvb", LengthUnit::Dvb),
            ("lvw", LengthUnit::Lvw),
            ("lvh", LengthUnit::Lvh),
            ("lvi", LengthUnit::Lvi),
            ("lvb", LengthUnit::Lvb),
            ("rem", LengthUnit::Rem),
            ("svw", LengthUnit::Svw),
            ("svh", LengthUnit::Svh),
            ("svi", LengthUnit::Svi),
            ("svb", LengthUnit::Svb),
            ("vw", LengthUnit::Vw),
            ("vh", LengthUnit::Vh),
            ("vi", LengthUnit::Vi),
            ("vb", LengthUnit::Vb),
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
    pub(crate) relative_meta: [u32; 2],
    pub(crate) relative_values: [f32; MAX_CALC_RELATIVE_TERMS],
}

impl CalcLengthPercentage {
    pub(crate) fn add_relative(&mut self, unit: LengthUnit, value: f32) -> Result<(), ()> {
        let index = unit
            .relative_index()
            .expect("only relative units enter the relative calc table");
        let len = self.relative_len();
        if let Some(position) = (0..len).position(|stored| self.relative_index_at(stored) == index)
        {
            self.relative_values[position] += value;
            if self.relative_values[position] == 0.0 {
                self.remove_relative(position);
            }
            return Ok(());
        }
        if value == 0.0 {
            return Ok(());
        }
        if len == MAX_CALC_RELATIVE_TERMS {
            return Err(());
        }
        let insert_at = (0..len)
            .find(|stored| self.relative_index_at(*stored) >= index)
            .unwrap_or(len);
        for position in (insert_at..len).rev() {
            self.set_relative_index(position + 1, self.relative_index_at(position));
        }
        self.relative_values
            .copy_within(insert_at..len, insert_at + 1);
        self.set_relative_index(insert_at, index);
        self.relative_values[insert_at] = value;
        self.set_relative_len(len + 1);
        Ok(())
    }

    pub(crate) fn add_relative_terms(&mut self, other: Self) -> Result<(), ()> {
        for (unit, value) in other.relative_terms() {
            self.add_relative(unit, value)?;
        }
        Ok(())
    }

    pub(crate) fn scale_relative(&mut self, factor: f32) {
        let len = self.relative_len();
        for value in &mut self.relative_values[..len] {
            *value *= factor;
        }
    }

    pub(crate) fn relative_is_finite(self) -> bool {
        self.relative_values[..self.relative_len()]
            .iter()
            .all(|value| value.is_finite())
    }

    fn relative_terms(self) -> impl Iterator<Item = (LengthUnit, f32)> {
        let len = self.relative_len();
        (0..len).map(move |position| {
            let index = self.relative_index_at(position);
            (RELATIVE_UNITS[index], self.relative_values[position])
        })
    }

    fn relative_len(self) -> usize {
        (self.relative_meta() >> RELATIVE_LEN_SHIFT) as usize
    }

    fn set_relative_len(&mut self, len: usize) {
        let indices = self.relative_meta() & ((1_u64 << RELATIVE_LEN_SHIFT) - 1);
        self.set_relative_meta(indices | ((len as u64) << RELATIVE_LEN_SHIFT));
    }

    fn relative_index_at(self, position: usize) -> usize {
        ((self.relative_meta() >> (position * RELATIVE_INDEX_BITS)) & RELATIVE_INDEX_MASK) as usize
    }

    fn set_relative_index(&mut self, position: usize, index: usize) {
        let shift = position * RELATIVE_INDEX_BITS;
        let mut meta = self.relative_meta();
        meta &= !(RELATIVE_INDEX_MASK << shift);
        meta |= (index as u64) << shift;
        self.set_relative_meta(meta);
    }

    fn relative_meta(self) -> u64 {
        u64::from(self.relative_meta[0]) | (u64::from(self.relative_meta[1]) << 32)
    }

    fn set_relative_meta(&mut self, meta: u64) {
        self.relative_meta = [meta as u32, (meta >> 32) as u32];
    }

    fn remove_relative(&mut self, position: usize) {
        let len = self.relative_len();
        for stored in position + 1..len {
            self.set_relative_index(stored - 1, self.relative_index_at(stored));
        }
        self.relative_values
            .copy_within(position + 1..len, position);
        self.set_relative_index(len - 1, 0);
        self.relative_values[len - 1] = 0.0;
        self.set_relative_len(len - 1);
    }

    fn resolve_relative(mut self, environment: RelativeLengthEnvironment) -> Self {
        let mut position = 0;
        while position < self.relative_len() {
            let index = self.relative_index_at(position);
            let unit = RELATIVE_UNITS[index];
            let value = self.relative_values[position];
            if let Some(basis) = unit.relative_basis(environment) {
                self.px += value * basis / 100.0;
                self.remove_relative(position);
            } else {
                position += 1;
            }
        }
        self
    }

    fn has_unresolved_relative(self) -> bool {
        self.relative_len() != 0
    }
}

impl fmt::Display for CalcLengthPercentage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("calc(")?;
        let mut wrote = false;
        for (value, unit) in [
            (self.percentage * 100.0, "%"),
            (self.em, "em"),
            (self.px, "px"),
            (self.rem, "rem"),
        ] {
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
        for (unit, value) in self.relative_terms() {
            if wrote {
                formatter.write_str(if value.is_sign_negative() {
                    " - "
                } else {
                    " + "
                })?;
            } else if value.is_sign_negative() {
                formatter.write_str("-")?;
            }
            write!(formatter, "{}{}", format_number(value.abs()), unit.suffix())?;
            wrote = true;
        }
        if !wrote {
            formatter.write_str("0px")?;
        }
        formatter.write_str(")")
    }
}

pub(crate) const MAX_MATH_LEAVES: usize = 8;
pub(crate) const MAX_MATH_TOKENS: usize = 32;
const MATH_LEAF_BITS: usize = 6;
const MATH_LEAF_MASK: u64 = (1 << MATH_LEAF_BITS) - 1;
const MATH_ANGLE: u8 = 58;
const MATH_ROUNDING_STRATEGY: u8 = 59;
const MATH_NUMBER: u8 = 60;
const MATH_PERCENTAGE: u8 = 61;
const MATH_NONE: u8 = 62;
const MATH_TOKEN_BITS: usize = 5;
const MATH_TOKEN_MASK: u64 = (1 << MATH_TOKEN_BITS) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum RoundingStrategy {
    Nearest,
    Up,
    Down,
    ToZero,
}

impl fmt::Display for RoundingStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Nearest => "nearest",
            Self::Up => "up",
            Self::Down => "down",
            Self::ToZero => "to-zero",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MathOperand {
    Number(f32),
    Length(Length),
    Percentage(f32),
    /// Canonical radians. Angle units participate only inside a retained
    /// length-producing expression in the current property lane.
    Angle(f32),
    RoundingStrategy(RoundingStrategy),
    None,
}

impl MathOperand {
    fn has_percentage(self) -> bool {
        matches!(self, Self::Percentage(_))
    }

    fn resolve_font_relative(self, em: f32, rem: f32) -> Self {
        match self {
            Self::Length(length) if !length.unit.is_relative() => {
                Self::Length(Length::px(length.unit.to_px(length.value, em, rem)))
            },
            _ => self,
        }
    }

    fn resolve_relative(self, environment: RelativeLengthEnvironment) -> Self {
        match self {
            Self::Length(length) => Self::Length(length.resolve_relative(environment)),
            _ => self,
        }
    }
}

impl fmt::Display for MathOperand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => formatter.write_str(&format_number(*value)),
            Self::Length(length) => length.fmt(formatter),
            Self::Percentage(value) => write!(formatter, "{}%", format_number(value * 100.0)),
            Self::Angle(value) => write!(formatter, "{}rad", format_number(*value)),
            Self::RoundingStrategy(strategy) => strategy.fmt(formatter),
            Self::None => formatter.write_str("none"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MathOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Min,
    Max,
    Clamp,
    Round,
    Mod,
    Rem,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Sqrt,
    Hypot,
    Ln,
    Log,
    Exp,
    Abs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MathToken {
    Operand(u8),
    Operation(MathOperation),
}

impl MathToken {
    const fn code(self) -> u8 {
        match self {
            Self::Operand(index) => index,
            Self::Operation(MathOperation::Add) => 8,
            Self::Operation(MathOperation::Subtract) => 9,
            Self::Operation(MathOperation::Multiply) => 10,
            Self::Operation(MathOperation::Divide) => 11,
            Self::Operation(MathOperation::Min) => 12,
            Self::Operation(MathOperation::Max) => 13,
            Self::Operation(MathOperation::Clamp) => 14,
            Self::Operation(MathOperation::Round) => 15,
            Self::Operation(MathOperation::Mod) => 16,
            Self::Operation(MathOperation::Rem) => 17,
            Self::Operation(MathOperation::Sin) => 18,
            Self::Operation(MathOperation::Cos) => 19,
            Self::Operation(MathOperation::Tan) => 20,
            Self::Operation(MathOperation::Asin) => 21,
            Self::Operation(MathOperation::Acos) => 22,
            Self::Operation(MathOperation::Atan) => 23,
            Self::Operation(MathOperation::Atan2) => 24,
            Self::Operation(MathOperation::Pow) => 25,
            Self::Operation(MathOperation::Sqrt) => 26,
            Self::Operation(MathOperation::Hypot) => 27,
            Self::Operation(MathOperation::Ln) => 28,
            Self::Operation(MathOperation::Log) => 29,
            Self::Operation(MathOperation::Exp) => 30,
            Self::Operation(MathOperation::Abs) => 31,
        }
    }

    const fn from_code(code: u8) -> Self {
        match code {
            0..=7 => Self::Operand(code),
            8 => Self::Operation(MathOperation::Add),
            9 => Self::Operation(MathOperation::Subtract),
            10 => Self::Operation(MathOperation::Multiply),
            11 => Self::Operation(MathOperation::Divide),
            12 => Self::Operation(MathOperation::Min),
            13 => Self::Operation(MathOperation::Max),
            14 => Self::Operation(MathOperation::Clamp),
            15 => Self::Operation(MathOperation::Round),
            16 => Self::Operation(MathOperation::Mod),
            17 => Self::Operation(MathOperation::Rem),
            18 => Self::Operation(MathOperation::Sin),
            19 => Self::Operation(MathOperation::Cos),
            20 => Self::Operation(MathOperation::Tan),
            21 => Self::Operation(MathOperation::Asin),
            22 => Self::Operation(MathOperation::Acos),
            23 => Self::Operation(MathOperation::Atan),
            24 => Self::Operation(MathOperation::Atan2),
            25 => Self::Operation(MathOperation::Pow),
            26 => Self::Operation(MathOperation::Sqrt),
            27 => Self::Operation(MathOperation::Hypot),
            28 => Self::Operation(MathOperation::Ln),
            29 => Self::Operation(MathOperation::Log),
            30 => Self::Operation(MathOperation::Exp),
            31 => Self::Operation(MathOperation::Abs),
            _ => panic!("invalid math token"),
        }
    }

    const fn arity(self) -> usize {
        match self {
            Self::Operand(_) => 0,
            Self::Operation(MathOperation::Clamp | MathOperation::Round) => 3,
            Self::Operation(
                MathOperation::Sin
                | MathOperation::Cos
                | MathOperation::Tan
                | MathOperation::Asin
                | MathOperation::Acos
                | MathOperation::Atan
                | MathOperation::Sqrt
                | MathOperation::Ln
                | MathOperation::Exp
                | MathOperation::Abs,
            ) => 1,
            Self::Operation(_) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum EvaluatedMath {
    Value(f32),
    Strategy(RoundingStrategy),
    None,
}

/// A compact postfix math program retained until its font, viewport,
/// container, and percentage bases are all available.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathLengthPercentage {
    leaf_meta: [u32; 2],
    leaf_values: [f32; MAX_MATH_LEAVES],
    program: [u32; 5],
    leaf_len: u8,
    token_len: u8,
}

impl MathLengthPercentage {
    pub(crate) fn new(leaves: &[MathOperand], tokens: &[MathToken]) -> Result<Self, ParseError> {
        if leaves.is_empty()
            || leaves.len() > MAX_MATH_LEAVES
            || tokens.is_empty()
            || tokens.len() > MAX_MATH_TOKENS
        {
            return Err(ParseError::expected(
                "a math expression with at most eight leaves and thirty-two tokens",
            ));
        }
        let mut stored = Self {
            leaf_meta: [0; 2],
            leaf_values: [0.0; MAX_MATH_LEAVES],
            program: [0; 5],
            leaf_len: leaves.len() as u8,
            token_len: tokens.len() as u8,
        };
        for (index, operand) in leaves.iter().copied().enumerate() {
            stored.set_operand(index, operand);
        }
        for (index, token) in tokens.iter().copied().enumerate() {
            stored.set_token(index, token);
        }
        Ok(stored)
    }

    fn operand(self, index: usize) -> MathOperand {
        let code = ((join_u64(self.leaf_meta) >> (index * MATH_LEAF_BITS)) & MATH_LEAF_MASK) as u8;
        match code {
            MATH_NUMBER => MathOperand::Number(self.leaf_values[index]),
            MATH_PERCENTAGE => MathOperand::Percentage(self.leaf_values[index]),
            MATH_ANGLE => MathOperand::Angle(self.leaf_values[index]),
            MATH_ROUNDING_STRATEGY => MathOperand::RoundingStrategy(
                match self.leaf_values[index] as u8 {
                    0 => RoundingStrategy::Nearest,
                    1 => RoundingStrategy::Up,
                    2 => RoundingStrategy::Down,
                    3 => RoundingStrategy::ToZero,
                    _ => panic!("invalid rounding strategy"),
                },
            ),
            MATH_NONE => MathOperand::None,
            unit => MathOperand::Length(Length {
                value: self.leaf_values[index],
                unit: RELATIVE_AND_ABSOLUTE_UNITS[usize::from(unit)],
            }),
        }
    }

    fn set_operand(&mut self, index: usize, operand: MathOperand) {
        let (code, value) = match operand {
            MathOperand::Number(value) => (MATH_NUMBER, value),
            MathOperand::Length(length) => (length.unit as u8, length.value),
            MathOperand::Percentage(value) => (MATH_PERCENTAGE, value),
            MathOperand::Angle(value) => (MATH_ANGLE, value),
            MathOperand::RoundingStrategy(strategy) => {
                (MATH_ROUNDING_STRATEGY, strategy as u8 as f32)
            },
            MathOperand::None => (MATH_NONE, 0.0),
        };
        let shift = index * MATH_LEAF_BITS;
        let mut meta = join_u64(self.leaf_meta);
        meta &= !(MATH_LEAF_MASK << shift);
        meta |= u64::from(code) << shift;
        self.leaf_meta = split_u64(meta);
        self.leaf_values[index] = value;
    }

    fn tokens(self) -> impl Iterator<Item = MathToken> {
        (0..usize::from(self.token_len)).map(move |index| self.token(index))
    }

    fn token(self, index: usize) -> MathToken {
        let bit = index * MATH_TOKEN_BITS;
        let word = bit / u32::BITS as usize;
        let shift = bit % u32::BITS as usize;
        let low = u64::from(self.program[word]);
        let high = self
            .program
            .get(word + 1)
            .copied()
            .map(u64::from)
            .unwrap_or(0);
        MathToken::from_code((((low | (high << u32::BITS)) >> shift) & MATH_TOKEN_MASK) as u8)
    }

    fn set_token(&mut self, index: usize, token: MathToken) {
        let bit = index * MATH_TOKEN_BITS;
        let word = bit / u32::BITS as usize;
        let shift = bit % u32::BITS as usize;
        let encoded = u64::from(token.code()) << shift;
        self.program[word] |= encoded as u32;
        if shift + MATH_TOKEN_BITS > u32::BITS as usize {
            self.program[word + 1] |= (encoded >> u32::BITS) as u32;
        }
    }

    fn has_percentage(self) -> bool {
        (0..usize::from(self.leaf_len)).any(|index| self.operand(index).has_percentage())
    }

    pub(super) fn resolve_font_relative(mut self, em: f32, rem: f32) -> Self {
        for index in 0..usize::from(self.leaf_len) {
            self.set_operand(index, self.operand(index).resolve_font_relative(em, rem));
        }
        self
    }

    pub(super) fn resolve_relative(mut self, environment: RelativeLengthEnvironment) -> Self {
        for index in 0..usize::from(self.leaf_len) {
            self.set_operand(index, self.operand(index).resolve_relative(environment));
        }
        self
    }

    pub(super) fn resolved_px(self) -> Option<f32> {
        self.evaluate(|operand| match operand {
            MathOperand::Number(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::Length(Length {
                value,
                unit: LengthUnit::Px,
            }) => Some(EvaluatedMath::Value(value)),
            MathOperand::Angle(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::RoundingStrategy(strategy) => Some(EvaluatedMath::Strategy(strategy)),
            MathOperand::None => Some(EvaluatedMath::None),
            MathOperand::Length(_) | MathOperand::Percentage(_) => None,
        })
    }

    fn specified_absolute_px(self) -> Option<f32> {
        self.evaluate(|operand| match operand {
            MathOperand::Number(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::Length(length)
                if !length.unit.is_relative()
                    && !matches!(length.unit, LengthUnit::Em | LengthUnit::Rem) =>
            {
                Some(EvaluatedMath::Value(length.unit.to_px(
                    length.value,
                    0.0,
                    0.0,
                )))
            },
            MathOperand::Angle(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::RoundingStrategy(strategy) => Some(EvaluatedMath::Strategy(strategy)),
            MathOperand::None => Some(EvaluatedMath::None),
            MathOperand::Length(_) | MathOperand::Percentage(_) => None,
        })
    }

    fn to_px(self, em: f32, rem: f32, percentage_basis: f32) -> f32 {
        self.evaluate(|operand| match operand {
            MathOperand::Number(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::Length(length) => Some(EvaluatedMath::Value(length.unit.to_px(
                length.value,
                em,
                rem,
            ))),
            MathOperand::Percentage(value) => Some(EvaluatedMath::Value(value * percentage_basis)),
            MathOperand::Angle(value) => Some(EvaluatedMath::Value(value)),
            MathOperand::RoundingStrategy(strategy) => Some(EvaluatedMath::Strategy(strategy)),
            MathOperand::None => Some(EvaluatedMath::None),
        })
        .unwrap_or_else(|| {
            panic!(
                "validated math must evaluate after all bases are supplied: {}",
                self.expression_string()
                    .unwrap_or_else(|| "invalid postfix program".to_owned())
            )
        })
    }

    fn evaluate(
        self,
        mut resolve: impl FnMut(MathOperand) -> Option<EvaluatedMath>,
    ) -> Option<f32> {
        let mut stack = [EvaluatedMath::None; MAX_MATH_LEAVES];
        let mut len = 0;
        for token in self.tokens() {
            match token {
                MathToken::Operand(index) => {
                    stack[len] = resolve(self.operand(usize::from(index)))?;
                    len += 1;
                },
                MathToken::Operation(operation) => {
                    let arity = MathToken::Operation(operation).arity();
                    if len < arity {
                        return None;
                    }
                    let start = len - arity;
                    let result = match operation {
                        MathOperation::Clamp => {
                            let minimum = evaluated_bound(stack[start], f32::NEG_INFINITY)?;
                            let value = evaluated_value(stack[start + 1])?;
                            let maximum = evaluated_bound(stack[start + 2], f32::INFINITY)?;
                            EvaluatedMath::Value(value.min(maximum).max(minimum))
                        },
                        MathOperation::Round => {
                            let strategy = evaluated_strategy(stack[start])?;
                            let value = evaluated_value(stack[start + 1])?;
                            let step = evaluated_value(stack[start + 2])?.abs();
                            if step == 0.0 {
                                return None;
                            }
                            let quotient = value / step;
                            let rounded = match strategy {
                                RoundingStrategy::Nearest => (quotient + 0.5).floor(),
                                RoundingStrategy::Up => quotient.ceil(),
                                RoundingStrategy::Down => quotient.floor(),
                                RoundingStrategy::ToZero => quotient.trunc(),
                            };
                            EvaluatedMath::Value(rounded * step)
                        },
                        MathOperation::Sin
                        | MathOperation::Cos
                        | MathOperation::Tan
                        | MathOperation::Asin
                        | MathOperation::Acos
                        | MathOperation::Atan
                        | MathOperation::Sqrt
                        | MathOperation::Ln
                        | MathOperation::Exp
                        | MathOperation::Abs => {
                            let value = evaluated_value(stack[start])?;
                            EvaluatedMath::Value(match operation {
                                MathOperation::Sin => value.sin(),
                                MathOperation::Cos => value.cos(),
                                MathOperation::Tan => value.tan(),
                                MathOperation::Asin => value.asin(),
                                MathOperation::Acos => value.acos(),
                                MathOperation::Atan => value.atan(),
                                MathOperation::Sqrt => value.sqrt(),
                                MathOperation::Ln => value.ln(),
                                MathOperation::Exp => value.exp(),
                                MathOperation::Abs => value.abs(),
                                _ => unreachable!(),
                            })
                        },
                        operation => {
                            let left = evaluated_value(stack[start])?;
                            let right = evaluated_value(stack[start + 1])?;
                            let value = match operation {
                                MathOperation::Add => left + right,
                                MathOperation::Subtract => left - right,
                                MathOperation::Multiply => left * right,
                                MathOperation::Divide if right != 0.0 => left / right,
                                MathOperation::Divide => return None,
                                MathOperation::Min => left.min(right),
                                MathOperation::Max => left.max(right),
                                MathOperation::Mod if right != 0.0 => {
                                    left - right * (left / right).floor()
                                },
                                MathOperation::Rem if right != 0.0 => {
                                    left - right * (left / right).trunc()
                                },
                                MathOperation::Mod | MathOperation::Rem => return None,
                                MathOperation::Atan2 => left.atan2(right),
                                MathOperation::Pow => left.powf(right),
                                MathOperation::Hypot => left.hypot(right),
                                MathOperation::Log => left.log(right),
                                MathOperation::Clamp
                                | MathOperation::Round
                                | MathOperation::Sin
                                | MathOperation::Cos
                                | MathOperation::Tan
                                | MathOperation::Asin
                                | MathOperation::Acos
                                | MathOperation::Atan
                                | MathOperation::Sqrt
                                | MathOperation::Ln
                                | MathOperation::Exp
                                | MathOperation::Abs => unreachable!(),
                            };
                            EvaluatedMath::Value(value)
                        },
                    };
                    stack[start] = result;
                    len = start + 1;
                },
            }
        }
        if len != 1 {
            return None;
        }
        let value = evaluated_value(stack[0])?;
        value.is_finite().then_some(value)
    }

    fn expression_string(self) -> Option<String> {
        let mut stack = Vec::with_capacity(MAX_MATH_LEAVES);
        for token in self.tokens() {
            match token {
                MathToken::Operand(index) => {
                    stack.push(self.operand(usize::from(index)).to_string())
                },
                MathToken::Operation(operation) => {
                    let arity = MathToken::Operation(operation).arity();
                    if stack.len() < arity {
                        return None;
                    }
                    let start = stack.len() - arity;
                    let result = match operation {
                        MathOperation::Clamp => format!(
                            "clamp({}, {}, {})",
                            stack[start],
                            stack[start + 1],
                            stack[start + 2]
                        ),
                        MathOperation::Round => format!(
                            "round({}, {}, {})",
                            stack[start],
                            stack[start + 1],
                            stack[start + 2]
                        ),
                        MathOperation::Add
                        | MathOperation::Subtract
                        | MathOperation::Multiply
                        | MathOperation::Divide => {
                            let separator = match operation {
                                MathOperation::Add => " + ",
                                MathOperation::Subtract => " - ",
                                MathOperation::Multiply => " * ",
                                MathOperation::Divide => " / ",
                                _ => unreachable!(),
                            };
                            format!("calc({}{separator}{})", stack[start], stack[start + 1])
                        },
                        operation => {
                            let function = match operation {
                                MathOperation::Min => "min",
                                MathOperation::Max => "max",
                                MathOperation::Mod => "mod",
                                MathOperation::Rem => "rem",
                                MathOperation::Sin => "sin",
                                MathOperation::Cos => "cos",
                                MathOperation::Tan => "tan",
                                MathOperation::Asin => "asin",
                                MathOperation::Acos => "acos",
                                MathOperation::Atan => "atan",
                                MathOperation::Atan2 => "atan2",
                                MathOperation::Pow => "pow",
                                MathOperation::Sqrt => "sqrt",
                                MathOperation::Hypot => "hypot",
                                MathOperation::Ln | MathOperation::Log => "log",
                                MathOperation::Exp => "exp",
                                MathOperation::Abs => "hypot",
                                _ => unreachable!(),
                            };
                            let arguments = stack[start..]
                                .iter()
                                .take(arity)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("{function}({arguments})")
                        },
                    };
                    stack.truncate(start);
                    stack.push(result);
                },
            }
        }
        (stack.len() == 1).then(|| stack.remove(0))
    }
}

fn evaluated_value(value: EvaluatedMath) -> Option<f32> {
    match value {
        EvaluatedMath::Value(value) => Some(value),
        EvaluatedMath::Strategy(_) | EvaluatedMath::None => None,
    }
}

fn evaluated_strategy(value: EvaluatedMath) -> Option<RoundingStrategy> {
    match value {
        EvaluatedMath::Strategy(strategy) => Some(strategy),
        EvaluatedMath::Value(_) | EvaluatedMath::None => None,
    }
}

fn evaluated_bound(value: EvaluatedMath, none: f32) -> Option<f32> {
    match value {
        EvaluatedMath::Value(value) => Some(value),
        EvaluatedMath::None => Some(none),
        EvaluatedMath::Strategy(_) => None,
    }
}

const RELATIVE_AND_ABSOLUTE_UNITS: [LengthUnit; 39] = [
    LengthUnit::Px,
    LengthUnit::Em,
    LengthUnit::Rem,
    LengthUnit::In,
    LengthUnit::Cm,
    LengthUnit::Mm,
    LengthUnit::Q,
    LengthUnit::Pt,
    LengthUnit::Pc,
    LengthUnit::Vw,
    LengthUnit::Vh,
    LengthUnit::Vi,
    LengthUnit::Vb,
    LengthUnit::Vmin,
    LengthUnit::Vmax,
    LengthUnit::Svw,
    LengthUnit::Svh,
    LengthUnit::Svi,
    LengthUnit::Svb,
    LengthUnit::Svmin,
    LengthUnit::Svmax,
    LengthUnit::Lvw,
    LengthUnit::Lvh,
    LengthUnit::Lvi,
    LengthUnit::Lvb,
    LengthUnit::Lvmin,
    LengthUnit::Lvmax,
    LengthUnit::Dvw,
    LengthUnit::Dvh,
    LengthUnit::Dvi,
    LengthUnit::Dvb,
    LengthUnit::Dvmin,
    LengthUnit::Dvmax,
    LengthUnit::Cqw,
    LengthUnit::Cqh,
    LengthUnit::Cqi,
    LengthUnit::Cqb,
    LengthUnit::Cqmin,
    LengthUnit::Cqmax,
];

const fn join_u64(value: [u32; 2]) -> u64 {
    value[0] as u64 | ((value[1] as u64) << 32)
}

const fn split_u64(value: u64) -> [u32; 2] {
    [value as u32, (value >> 32) as u32]
}

impl fmt::Display for MathLengthPercentage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(value) = self.specified_absolute_px() {
            return write!(formatter, "calc({}px)", format_number(value));
        }
        formatter.write_str(
            &self
                .expression_string()
                .unwrap_or_else(|| "calc(0px)".to_owned()),
        )
    }
}

/// A length, percentage, linear `calc()`, or bounded comparison function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LengthPercentage {
    Zero,
    Length(Length),
    /// Stored as a unit value: `1.0` is `100%`.
    Percentage(f32),
    Calc(CalcLengthPercentage),
    Math(MathLengthPercentage),
}

impl LengthPercentage {
    pub const ZERO: Self = Self::Zero;

    /// Whether resolving this value needs a percentage basis supplied by the
    /// consuming property.
    pub fn has_percentage(self) -> bool {
        match self {
            Self::Percentage(_) => true,
            Self::Calc(calc) => calc.percentage != 0.0,
            Self::Math(math) => math.has_percentage(),
            Self::Zero | Self::Length(_) => false,
        }
    }

    /// Resolve absolute and font-relative terms while preserving any
    /// percentage for the property's later used-value basis.
    pub fn resolve_font_relative(self, em: f32, rem: f32) -> Self {
        match self {
            Self::Zero | Self::Percentage(_) => self,
            Self::Length(length) if !length.unit.is_relative() => {
                Self::Length(Length::px(length.unit.to_px(length.value, em, rem)))
            },
            Self::Length(_) => self,
            Self::Calc(calc) => {
                let resolved = CalcLengthPercentage {
                    percentage: calc.percentage,
                    px: calc.px + calc.em * em + calc.rem * rem,
                    em: 0.0,
                    rem: 0.0,
                    ..calc
                };
                if resolved.percentage == 0.0 && !resolved.has_unresolved_relative() {
                    Self::Length(Length::px(resolved.px))
                } else {
                    Self::Calc(resolved)
                }
            },
            Self::Math(math) => {
                let resolved = math.resolve_font_relative(em, rem);
                resolved
                    .resolved_px()
                    .map_or(Self::Math(resolved), |value| {
                        Self::Length(Length::px(value))
                    })
            },
        }
    }

    /// Resolve viewport-relative terms against the host's current device while
    /// preserving percentages and font-relative terms for their later bases.
    pub fn resolve_viewport(self, viewport_width: f32, viewport_height: f32) -> Self {
        self.resolve_relative(RelativeLengthEnvironment::uniform_viewport(
            viewport_width,
            viewport_height,
        ))
    }

    /// Resolve every relative unit whose environmental basis is available.
    /// Container units remain authored during cascade and resolve after the
    /// preliminary layout identifies eligible ancestor content boxes.
    pub fn resolve_relative(self, environment: RelativeLengthEnvironment) -> Self {
        match self {
            Self::Length(length) => Self::Length(length.resolve_relative(environment)),
            Self::Calc(calc) => Self::Calc(calc.resolve_relative(environment)),
            Self::Math(math) => {
                let resolved = math.resolve_relative(environment);
                resolved
                    .resolved_px()
                    .map_or(Self::Math(resolved), |value| {
                        Self::Length(Length::px(value))
                    })
            },
            Self::Zero | Self::Percentage(_) => self,
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
                assert!(
                    !calc.has_unresolved_relative(),
                    "environment-relative calc terms must resolve before px consumption"
                );
                calc.percentage * percentage_basis + calc.px + calc.em * em + calc.rem * rem
            },
            Self::Math(math) => math.to_px(em, rem, percentage_basis),
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
            return super::calc::parse_length_percentage(input)
                .map(Self::Calc)
                .or_else(|_| super::calc::parse_math(input).map(Self::Math));
        }
        if [
            "min(", "max(", "clamp(", "round(", "mod(", "rem(", "sin(", "cos(", "tan(",
            "asin(", "acos(", "atan(", "atan2(", "pow(", "sqrt(", "hypot(", "log(", "exp(",
        ]
        .iter()
        .any(|function| {
            input
                .get(..function.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(function))
        }) {
            return super::calc::parse_math(input).map(Self::Math);
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
            Self::Math(math) => math.fmt(formatter),
        }
    }
}
