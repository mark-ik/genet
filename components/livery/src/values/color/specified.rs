/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The specified-value form of a color.
//!
//! CSSOM's `getPropertyValue()` returns the *specified* value, and CSS keeps
//! more of the authored shape there than the computed value does: named
//! keywords stay keywords, and `color-mix()` and relative colors serialize as
//! themselves rather than as their resolved color
//! (<https://github.com/w3c/csswg-drafts/issues/7302>). Livery's [`Color`] is
//! the computed form and resolves eagerly, which is correct for the cascade
//! and paint but wrong for `e.style` reads; this type is the retained layer
//! in between.
//!
//! Validation is the resolving parser's job: a string only becomes a
//! `SpecifiedColor` if [`Color`] accepts it, so this module never widens the
//! grammar. It only remembers what the resolver forgets.

use std::{fmt, str::FromStr};

use cssparser::{Parser, ParserInput, Token};

use super::layers::BlendMode;
use super::{Color, ColorSpace, parse};
use crate::values::{ParseError, format_number};

/// A color as CSSOM's specified level serializes it.
#[derive(Clone, Debug, PartialEq)]
pub enum SpecifiedColor {
    /// A bare keyword: a named color, `transparent`, `currentcolor`, or a
    /// system color. Keywords round-trip at the specified level.
    Keyword(String),
    /// Everything that resolves at parse time and serializes resolved:
    /// hex and the absolute color functions.
    Absolute(Color),
    Relative(RelativeForm),
    Mix(MixForm),
    Alpha(AlphaForm),
    Contrast(ContrastForm),
    Layers(LayersForm),
}

/// A retained `rgb(from ...)`-family value.
#[derive(Clone, Debug, PartialEq)]
pub struct RelativeForm {
    /// Canonical function name (`rgba` has already become `rgb`).
    function: String,
    /// The `color()` color-space ident, authored spelling, lowercased.
    space: Option<String>,
    origin: Box<SpecifiedColor>,
    channels: [String; 3],
    alpha: Option<String>,
}

/// A retained `color-mix()` value.
#[derive(Clone, Debug, PartialEq)]
pub struct MixForm {
    /// The interpolation-space ident, authored spelling, lowercased.
    space: String,
    /// The hue method when it is not the `shorter` default, which serializes
    /// to nothing.
    hue: Option<String>,
    operands: [MixOperand; 2],
}

/// A retained `alpha()` value.
#[derive(Clone, Debug, PartialEq)]
pub struct AlphaForm {
    origin: Box<SpecifiedColor>,
    alpha: Option<String>,
}

/// A retained `contrast-color()` value.
#[derive(Clone, Debug, PartialEq)]
pub struct ContrastForm {
    origin: Box<SpecifiedColor>,
}

/// A retained `color-layers()` value.
#[derive(Clone, Debug, PartialEq)]
pub struct LayersForm {
    blend_mode: BlendMode,
    layers: Vec<SpecifiedColor>,
}

#[derive(Clone, Debug, PartialEq)]
struct MixOperand {
    color: Box<SpecifiedColor>,
    percentage: Option<Percentage>,
}

#[derive(Clone, Debug, PartialEq)]
enum Percentage {
    Number(f32),
    /// A retained math expression; its complement cannot be computed, so it
    /// serializes alone.
    Math(String),
}

type Failure<'i> = cssparser::ParseError<'i, ()>;

/// Undo the f32 noise of scaling a tokenizer unit value by 100: an authored
/// `30%` arrives as 0.30000001 and must not serialize as `30.000002%`. Four
/// decimals keeps every percentage an author can meaningfully write.
fn percent_value(unit_value: f32) -> f32 {
    ((f64::from(unit_value) * 100.0 * 10_000.0).round() / 10_000.0) as f32
}

impl SpecifiedColor {
    /// The resolved computed-form color, when one exists at parse time.
    pub fn resolved(&self) -> Option<Color> {
        match self {
            Self::Absolute(color) => Some(*color),
            // Keywords and retained functions re-resolve through the
            // computed parser; a keyword always succeeds.
            _ => self.to_string().parse::<Color>().ok(),
        }
    }
}

impl FromStr for SpecifiedColor {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let mut buffer = ParserInput::new(input);
        let mut parser = Parser::new(&mut buffer);
        match capture(&mut parser) {
            Ok(Some(captured)) if parser.expect_exhausted().is_ok() => Ok(captured),
            _ => {
                // Anything without a retained form is still validated by the
                // resolving parser and serializes resolved.
                let resolved = input.parse::<Color>()?;
                if !input.contains('(') && !input.starts_with('#') {
                    Ok(Self::Keyword(input.to_ascii_lowercase()))
                } else {
                    Ok(Self::Absolute(resolved))
                }
            },
        }
    }
}

/// Shallow structural capture. Returns `Ok(None)` for shapes with no retained
/// form; the caller falls back to the resolved color.
fn capture<'i>(input: &mut Parser<'i, '_>) -> Result<Option<SpecifiedColor>, Failure<'i>> {
    let name = match input.next()?.clone() {
        Token::Function(name) => name.to_string().to_ascii_lowercase(),
        _ => return Ok(None),
    };
    input.parse_nested_block(|inner| match name.as_str() {
        "color-mix" => capture_mix(inner).map(Some),
        "alpha" => capture_alpha(inner).map(Some),
        "contrast-color" => capture_contrast(inner).map(Some),
        "color-layers" => capture_layers(inner).map(Some),
        "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color" => {
            capture_relative(inner, &name)
        },
        _ => Ok(None),
    })
}

fn capture_relative<'i>(
    input: &mut Parser<'i, '_>,
    name: &str,
) -> Result<Option<SpecifiedColor>, Failure<'i>> {
    if input
        .try_parse(|i| i.expect_ident_matching("from"))
        .is_err()
    {
        // Not a relative color: an absolute function, no retained form.
        while input.next().is_ok() {}
        return Ok(None);
    }
    let origin = capture_origin(input)?;
    let space = if name == "color" {
        Some(input.expect_ident_cloned()?.to_ascii_lowercase())
    } else {
        None
    };
    let channels = [
        capture_component(input)?,
        capture_component(input)?,
        capture_component(input)?,
    ];
    let alpha = if input.try_parse(|i| i.expect_delim('/')).is_ok() {
        Some(capture_component(input)?)
    } else {
        None
    };
    let function = match name {
        "rgba" => "rgb",
        "hsla" => "hsl",
        other => other,
    };
    let form = RelativeForm {
        function: function.to_owned(),
        space,
        origin: Box::new(origin),
        channels,
        alpha,
    };
    validate_relative(&form, input)?;
    Ok(Some(SpecifiedColor::Relative(form)))
}

/// Validate the relative channels against a resolvable stand-in origin.
///
/// The origin's value does not change the grammar. Replacing it with red lets
/// the computed parser reject bad channel names and dimensions while the
/// specified form still retains a valid unresolved origin such as
/// `currentcolor`.
fn validate_relative<'i>(form: &RelativeForm, input: &Parser<'i, '_>) -> Result<(), Failure<'i>> {
    let mut candidate = format!("{}(from red", form.function);
    if let Some(space) = &form.space {
        candidate.push(' ');
        candidate.push_str(space);
    }
    for channel in &form.channels {
        candidate.push(' ');
        candidate.push_str(channel);
    }
    if let Some(alpha) = &form.alpha {
        candidate.push_str(" / ");
        candidate.push_str(alpha);
    }
    candidate.push(')');
    candidate
        .parse::<Color>()
        .map(|_| ())
        .map_err(|_| input.new_custom_error(()))
}

fn capture_mix<'i>(input: &mut Parser<'i, '_>) -> Result<SpecifiedColor, Failure<'i>> {
    let interpolation = input
        .try_parse(|nested| {
            nested.expect_ident_matching("in")?;
            let space = nested.expect_ident_cloned()?.to_ascii_lowercase();
            let hue = nested
                .try_parse(|i| {
                    let method = i.expect_ident_cloned()?.to_ascii_lowercase();
                    i.expect_ident_matching("hue")?;
                    Ok::<_, Failure<'i>>(method)
                })
                .ok()
                .filter(|method| method != "shorter");
            nested.expect_comma()?;
            Ok::<_, Failure<'i>>((space, hue))
        })
        .ok();
    let (space, hue) = interpolation.unwrap_or_else(|| ("oklab".to_owned(), None));
    let first = capture_operand(input)?;
    input.expect_comma()?;
    let second = capture_operand(input)?;
    let form = MixForm {
        space,
        hue,
        operands: [first, second],
    };
    validate_mix(&form, input)?;
    Ok(SpecifiedColor::Mix(form))
}

fn validate_mix<'i>(form: &MixForm, input: &Parser<'i, '_>) -> Result<(), Failure<'i>> {
    let space = ColorSpace::from_interpolation_name(&form.space)
        .ok_or_else(|| input.new_custom_error(()))?;
    if form.hue.is_some() && space.hue_index().is_none() {
        return Err(input.new_custom_error(()));
    }
    if let Some(hue) = &form.hue
        && !matches!(
            hue.as_str(),
            "longer" | "increasing" | "decreasing" | "shorter"
        )
    {
        return Err(input.new_custom_error(()));
    }

    let mut candidate = if form.space == "oklab" {
        "color-mix(".to_owned()
    } else {
        format!("color-mix(in {}", form.space)
    };
    if form.space != "oklab" {
        if let Some(hue) = &form.hue {
            candidate.push(' ');
            candidate.push_str(hue);
            candidate.push_str(" hue");
        }
        candidate.push_str(", ");
    }
    for (index, operand) in form.operands.iter().enumerate() {
        if index != 0 {
            candidate.push_str(", ");
        }
        candidate.push_str(if index == 0 { "red" } else { "blue" });
        if let Some(percentage) = &operand.percentage {
            candidate.push(' ');
            candidate.push_str(&percentage.to_css());
        }
    }
    candidate.push(')');
    candidate
        .parse::<Color>()
        .map(|_| ())
        .map_err(|_| input.new_custom_error(()))
}

fn capture_alpha<'i>(input: &mut Parser<'i, '_>) -> Result<SpecifiedColor, Failure<'i>> {
    input.expect_ident_matching("from")?;
    let origin = capture_origin(input)?;
    let alpha = if input.try_parse(|nested| nested.expect_delim('/')).is_ok() {
        Some(canonicalize_alpha_component(capture_component(input)?))
    } else {
        None
    };
    let form = AlphaForm {
        origin: Box::new(origin),
        alpha,
    };
    validate_alpha(&form, input)?;
    Ok(SpecifiedColor::Alpha(form))
}

/// CSS math serialization places a constant before `alpha` for commutative
/// binary addition and multiplication. The retained math program does not
/// carry symbolic channel keywords yet, so canonicalize this bounded shape
/// before preserving all other valid expressions verbatim.
fn canonicalize_alpha_component(source: String) -> String {
    let lower = source.to_ascii_lowercase();
    let Some(body) = lower
        .strip_prefix("calc(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return source;
    };
    let compact: String = body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    for operator in ['*', '+'] {
        let prefix = format!("alpha{operator}");
        if let Some(number) = compact.strip_prefix(&prefix)
            && let Ok(value) = number.parse::<f32>()
            && value.is_finite()
        {
            return format!("calc({} {operator} alpha)", format_number(value));
        }
    }
    source
}

fn validate_alpha<'i>(form: &AlphaForm, input: &Parser<'i, '_>) -> Result<(), Failure<'i>> {
    let mut candidate = "alpha(from red".to_owned();
    if let Some(alpha) = &form.alpha {
        candidate.push_str(" / ");
        candidate.push_str(alpha);
    }
    candidate.push(')');
    candidate
        .parse::<Color>()
        .map(|_| ())
        .map_err(|_| input.new_custom_error(()))
}

fn capture_contrast<'i>(input: &mut Parser<'i, '_>) -> Result<SpecifiedColor, Failure<'i>> {
    Ok(SpecifiedColor::Contrast(ContrastForm {
        origin: Box::new(capture_origin(input)?),
    }))
}

fn capture_layers<'i>(input: &mut Parser<'i, '_>) -> Result<SpecifiedColor, Failure<'i>> {
    let blend_mode = input
        .try_parse(|nested| {
            let name = nested.expect_ident_cloned()?;
            let mode =
                BlendMode::from_name(&name).ok_or_else(|| nested.new_custom_error::<(), ()>(()))?;
            nested.expect_comma()?;
            Ok::<_, Failure<'i>>(mode)
        })
        .unwrap_or(BlendMode::Normal);
    let mut layers = vec![capture_origin(input)?];
    while input.try_parse(|nested| nested.expect_comma()).is_ok() {
        layers.push(capture_origin(input)?);
    }
    Ok(SpecifiedColor::Layers(LayersForm { blend_mode, layers }))
}

fn capture_operand<'i>(input: &mut Parser<'i, '_>) -> Result<MixOperand, Failure<'i>> {
    let leading = input.try_parse(capture_percentage).ok();
    let color = capture_origin(input)?;
    let percentage = match leading {
        Some(value) => Some(value),
        None => input.try_parse(capture_percentage).ok(),
    };
    Ok(MixOperand {
        color: Box::new(color),
        percentage,
    })
}

fn capture_percentage<'i>(input: &mut Parser<'i, '_>) -> Result<Percentage, Failure<'i>> {
    let start = input.state();
    let position = input.position();
    match input.next()?.clone() {
        Token::Percentage { unit_value, .. } => {
            let value = percent_value(unit_value);
            if !(0.0..=100.0).contains(&value) {
                return Err(input.new_custom_error(()));
            }
            Ok(Percentage::Number(value))
        },
        // Only the math functions may stand in for a percentage; anything
        // else here is the operand's color (`hsl(...)` must not be eaten).
        Token::Function(name) if parse::is_math_function(&name) => {
            input.parse_nested_block(|inner| {
                while inner.next().is_ok() {}
                Ok::<(), Failure<'i>>(())
            })?;
            Ok(Percentage::Math(
                input.slice_from(position).trim().to_owned(),
            ))
        },
        _ => {
            input.reset(&start);
            Err(input.new_custom_error(()))
        },
    }
}

/// One color argument, advanced by the resolving parser (which validates it)
/// and captured as its source slice, then recursively given a specified form.
fn capture_origin<'i>(input: &mut Parser<'i, '_>) -> Result<SpecifiedColor, Failure<'i>> {
    let position = input.position();
    let start = input.state();
    match capture(input) {
        Ok(Some(captured)) => return Ok(captured),
        Ok(None) => input.reset(&start),
        Err(error) => return Err(error),
    }
    parse::parse_from(input)?;
    let source = input.slice_from(position).trim();
    source
        .parse::<SpecifiedColor>()
        .map_err(|_| input.new_custom_error(()))
}

/// One channel value, canonicalized token-by-token: idents lowercase, numbers
/// and percentages through the shared formatter, math functions kept whole.
fn capture_component<'i>(input: &mut Parser<'i, '_>) -> Result<String, Failure<'i>> {
    let position = input.position();
    match input.next()?.clone() {
        Token::Ident(ident) => Ok(ident.to_ascii_lowercase()),
        Token::Number { value, .. } => Ok(format_number(value)),
        Token::Percentage { unit_value, .. } => {
            Ok(format!("{}%", format_number(percent_value(unit_value))))
        },
        Token::Dimension { value, unit, .. } => Ok(format!(
            "{}{}",
            format_number(value),
            unit.to_ascii_lowercase()
        )),
        Token::Function(_) => {
            input.parse_nested_block(|inner| {
                while inner.next().is_ok() {}
                Ok::<(), Failure<'i>>(())
            })?;
            Ok(input.slice_from(position).trim().to_owned())
        },
        _ => Err(input.new_custom_error(())),
    }
}

impl fmt::Display for SpecifiedColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keyword(keyword) => formatter.write_str(keyword),
            Self::Absolute(color) => color.fmt(formatter),
            Self::Relative(form) => form.fmt(formatter),
            Self::Mix(form) => form.fmt(formatter),
            Self::Alpha(form) => form.fmt(formatter),
            Self::Contrast(form) => form.fmt(formatter),
            Self::Layers(form) => form.fmt(formatter),
        }
    }
}

impl fmt::Display for RelativeForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}(from {}", self.function, self.origin)?;
        if let Some(space) = &self.space {
            write!(formatter, " {space}")?;
        }
        for channel in &self.channels {
            write!(formatter, " {channel}")?;
        }
        if let Some(alpha) = &self.alpha {
            write!(formatter, " / {alpha}")?;
        }
        formatter.write_str(")")
    }
}

impl fmt::Display for MixForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("color-mix(")?;
        if self.space != "oklab" {
            write!(formatter, "in {}", self.space)?;
            if let Some(hue) = &self.hue {
                write!(formatter, " {hue} hue")?;
            }
            formatter.write_str(",")?;
        }
        // CSS Color 5 serialization: percentages print explicitly, with an
        // omitted one filled in as the other's complement, except in the
        // balanced 50%/50% case, which prints neither. A math percentage has
        // no computable complement and prints alone.
        let [first, second] = &self.operands;
        let (left, right) = match (&first.percentage, &second.percentage) {
            (None, None) => (None, None),
            (Some(Percentage::Number(p)), None) | (None, Some(Percentage::Number(p)))
                if *p == 50.0 =>
            {
                (None, None)
            },
            (Some(Percentage::Number(p)), None) => {
                (Some(format_pct(*p)), Some(format_pct(100.0 - *p)))
            },
            (None, Some(Percentage::Number(p))) => {
                (Some(format_pct(100.0 - *p)), Some(format_pct(*p)))
            },
            (left, right) => (
                left.as_ref().map(Percentage::to_css),
                right.as_ref().map(Percentage::to_css),
            ),
        };
        if self.space == "oklab" {
            write!(formatter, "{}", first.color)?;
            if let Some(percentage) = left {
                write!(formatter, " {percentage}")?;
            }
        } else {
            write_operand(formatter, first, left)?;
        }
        formatter.write_str(",")?;
        write_operand(formatter, second, right)?;
        formatter.write_str(")")
    }
}

impl fmt::Display for AlphaForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "alpha(from {}", self.origin)?;
        if let Some(alpha) = &self.alpha {
            write!(formatter, " / {alpha}")?;
        }
        formatter.write_str(")")
    }
}

impl fmt::Display for ContrastForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "contrast-color({})", self.origin)
    }
}

impl fmt::Display for LayersForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("color-layers(")?;
        if self.blend_mode != BlendMode::Normal {
            write!(formatter, "{}, ", self.blend_mode)?;
        }
        for (index, layer) in self.layers.iter().enumerate() {
            if index != 0 {
                formatter.write_str(", ")?;
            }
            layer.fmt(formatter)?;
        }
        formatter.write_str(")")
    }
}

impl Percentage {
    fn to_css(&self) -> String {
        match self {
            Self::Number(value) => format_pct(*value),
            Self::Math(source) => source.clone(),
        }
    }
}

fn format_pct(value: f32) -> String {
    format!("{}%", format_number(value))
}

fn write_operand(
    formatter: &mut fmt::Formatter<'_>,
    operand: &MixOperand,
    percentage: Option<String>,
) -> fmt::Result {
    write!(formatter, " {}", operand.color)?;
    if let Some(percentage) = percentage {
        write!(formatter, " {percentage}")?;
    }
    Ok(())
}
