/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Color 4/5 function grammar.
//!
//! Harvest lift (cutover plan F0). Shaped after the stylo fork's
//! `style/color/parsing.rs` and `color_function.rs` at
//! `b157d925267fdd37b03f43e3387ab2f0909e57b0`, reduced to Livery's retained
//! model. `cssparser` supplies tokenization and the hex/named tables only; the
//! function grammar is the consumer's job, which is why this exists.
//!
//! Covers `rgb()`/`rgba()`, `hsl()`/`hsla()`, `hwb()`, `lab()`, `lch()`,
//! `oklab()`, `oklch()`, `color()`, and `color-mix()`, in both legacy
//! comma-separated and modern space-separated forms, with `none` components
//! and slash alpha.

use cssparser::color::{parse_hash_color, parse_named_color};
use cssparser::{BasicParseErrorKind, ParseError as CssParseError, Parser, ParserInput, Token};

use super::relative::{self, Bindings};
use super::space::{ColorSpace, Components, normalize_hue};
use super::Color;
use crate::values::ParseError;

type Failure<'i> = CssParseError<'i, ()>;

fn fail<'i>(input: &Parser<'i, '_>) -> Failure<'i> {
    input.new_custom_error(())
}

/// How a channel's number maps onto its space's range.
#[derive(Clone, Copy, PartialEq)]
enum Channel {
    /// 0-255 from a number, 0-100% from a percentage. Legacy rgb().
    Rgb8,
    /// Percentage and number both map to 0-1. Modern rgb() and color().
    Unit,
    /// A hue angle in degrees; accepts `<angle>` units.
    Hue,
    /// A percentage reference, e.g. hsl()'s saturation (100% = 100.0).
    Percent100,
    /// Lightness for lab()/lch(): percentage reference 100.
    LabLightness,
    /// Lightness for oklab()/oklch(): percentage reference 1.0.
    OklabLightness,
    /// lab()/lch() a, b, chroma: percentage reference 125.
    LabAxis,
    /// oklab()/oklch() a, b, chroma: percentage reference 0.4.
    OklabAxis,
}

impl Channel {
    fn percentage_basis(self) -> f32 {
        match self {
            Self::Rgb8 => 255.0,
            Self::Unit => 1.0,
            Self::Hue => 1.0,
            Self::Percent100 | Self::LabLightness => 100.0,
            Self::OklabLightness => 1.0,
            Self::LabAxis => 125.0,
            Self::OklabAxis => 0.4,
        }
    }
}

/// How a channel was written, which the legacy forms constrain.
#[derive(Clone, Copy, PartialEq)]
enum Written {
    Number,
    Percentage,
    /// `none`, a keyword, or a math function: never valid in a legacy form,
    /// so its exact spelling does not need distinguishing.
    Other,
}

/// One parsed channel. `None` is CSS `none`, carried as a missing component.
fn parse_channel<'i>(
    input: &mut Parser<'i, '_>,
    channel: Channel,
    bindings: Option<&Bindings>,
) -> Result<(Option<f32>, Written), Failure<'i>> {
    // `none` is a plain ident and must be checked before the number grammar,
    // as must a bound channel keyword in a relative color.
    let start = input.state();
    if let Ok(ident) = input.expect_ident_cloned() {
        if ident.eq_ignore_ascii_case("none") {
            return Ok((None, Written::Other));
        }
        if let Some(bindings) = bindings
            && let Some(value) = relative::lookup(bindings, &ident)
        {
            return Ok((Some(value), Written::Other));
        }
    }
    input.reset(&start);

    if channel == Channel::Hue {
        return parse_hue(input, bindings).map(|hue| (Some(hue), Written::Number));
    }

    let location = input.current_source_location();
    let (value, written) = match input.next()?.clone() {
        Token::Number { value, .. } => (value, Written::Number),
        Token::Percentage { unit_value, .. } => (
            unit_value * channel.percentage_basis(),
            Written::Percentage,
        ),
        Token::Function(name) => {
            input.reset(&start);
            (
                parse_math_number(input, &name, channel, bindings)?,
                Written::Other,
            )
        },
        _ => return Err(location.new_custom_error(())),
    };
    if !value.is_finite() {
        return Err(fail(input));
    }
    Ok((Some(value), written))
}

/// A hue: `<number>` or `<angle>`.
fn parse_hue<'i>(
    input: &mut Parser<'i, '_>,
    bindings: Option<&Bindings>,
) -> Result<f32, Failure<'i>> {
    let start = input.state();
    let location = input.current_source_location();
    let degrees = match input.next()?.clone() {
        Token::Number { value, .. } => value,
        Token::Dimension { value, unit, .. } => match () {
            _ if unit.eq_ignore_ascii_case("deg") => value,
            _ if unit.eq_ignore_ascii_case("grad") => value * 360.0 / 400.0,
            _ if unit.eq_ignore_ascii_case("rad") => value.to_degrees(),
            _ if unit.eq_ignore_ascii_case("turn") => value * 360.0,
            _ => return Err(location.new_custom_error(())),
        },
        Token::Function(name) => {
            input.reset(&start);
            parse_math_number(input, &name, Channel::Hue, bindings)?
        },
        _ => return Err(location.new_custom_error(())),
    };
    if !degrees.is_finite() {
        return Err(fail(input));
    }
    Ok(degrees)
}

/// A `calc()`-family function in channel position.
///
/// Delegates to Livery's retained math program, which is dimensional: a hue
/// channel resolves as an `<angle>` (radians, converted back to degrees here),
/// every other channel as a `<number>`.
///
/// Known gap: a percentage-valued expression in channel position
/// (`rgb(calc(50%) 0 0)`) is rejected rather than scaled by the channel's
/// basis. The math program reduces percentages against a length base, which is
/// the wrong reference for a color channel. Recorded rather than approximated,
/// since a silent wrong scale is worse than a parse failure.
fn parse_math_number<'i>(
    input: &mut Parser<'i, '_>,
    name: &str,
    channel: Channel,
    bindings: Option<&Bindings>,
) -> Result<f32, Failure<'i>> {
    if !is_math_function(name) {
        return Err(fail(input));
    }
    let start_position = input.position();
    // Consume the whole function so the outer parser advances past it.
    input.next()?;
    input.parse_nested_block(|inner| {
        while inner.next().is_ok() {}
        Ok::<(), Failure<'i>>(())
    })?;
    let source = input.slice_from(start_position);
    // Livery's math program has no channel keywords, so a relative color
    // substitutes their values into the expression before parsing it.
    let substituted;
    let source = match bindings {
        Some(bindings) => {
            substituted = relative::substitute(source, bindings);
            substituted.as_str()
        },
        None => source,
    };
    if channel == Channel::Hue {
        // A hue expression may be angle-typed (`calc(30deg + 90deg)`) or
        // plain-number-typed (`calc(60 + 60)`, and every relative-color
        // expression, since the `h` keyword resolves to a number). Try the
        // angle reading first, then read a bare number as degrees.
        return crate::values::calc::parse_angle(source)
            .map(f32::to_degrees)
            .or_else(|_| crate::values::calc::parse_number(source))
            .map_err(|_| fail(input));
    }
    crate::values::calc::parse_number(source).map_err(|_| fail(input))
}

pub(super) fn is_math_function(name: &str) -> bool {
    const MATH: &[&str] = &[
        "calc", "min", "max", "clamp", "round", "mod", "rem", "sin", "cos", "tan", "asin", "acos",
        "atan", "atan2", "pow", "sqrt", "hypot", "log", "exp", "abs", "sign",
    ];
    MATH.iter().any(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// The alpha channel: `<number>` or `<percentage>`, or `none`.
pub(super) fn parse_alpha<'i>(
    input: &mut Parser<'i, '_>,
    bindings: Option<&Bindings>,
) -> Result<Option<f32>, Failure<'i>> {
    let start = input.state();
    if let Ok(ident) = input.expect_ident_cloned() {
        if ident.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        if let Some(bindings) = bindings
            && let Some(value) = relative::lookup(bindings, &ident)
        {
            return Ok(Some(value.clamp(0.0, 1.0)));
        }
    }
    input.reset(&start);
    let location = input.current_source_location();
    let value = match input.next()?.clone() {
        Token::Number { value, .. } => value,
        Token::Percentage { unit_value, .. } => unit_value,
        Token::Function(name) => {
            input.reset(&start);
            parse_math_number(input, &name, Channel::Unit, bindings)?
        },
        _ => return Err(location.new_custom_error(())),
    };
    if !value.is_finite() {
        return Err(fail(input));
    }
    Ok(Some(value.clamp(0.0, 1.0)))
}

/// The three channels plus optional alpha of a modern color function.
///
/// Modern syntax is space separated with an optional `/ <alpha>`; legacy
/// syntax is comma separated and forbids `none`. A function is legacy only if
/// a comma follows the first channel.
struct Parsed {
    components: Components,
    alpha: Option<f32>,
    legacy: bool,
}

fn parse_components<'i>(
    input: &mut Parser<'i, '_>,
    channels: [Channel; 3],
    bindings: Option<&Bindings>,
) -> Result<Parsed, Failure<'i>> {
    let (first, first_written) = parse_channel(input, channels[0], bindings)?;
    // A relative color is always modern syntax: `rgb(from red r, g, b)` is
    // invalid, so a comma is a failure there rather than a legacy form.
    let legacy = bindings.is_none() && input.try_parse(|i| i.expect_comma()).is_ok();

    let (second, second_written) = parse_channel(input, channels[1], bindings)?;
    if legacy {
        input.expect_comma()?;
    }
    let (third, third_written) = parse_channel(input, channels[2], bindings)?;

    // The legacy comma forms are type-uniform, which the modern forms are
    // not: `rgb(10%, 20, 30%)` and `rgba(-2, 300, 400%, -0.5)` are both
    // invalid for mixing percentages with numbers, even though every channel
    // would clamp into range on its own. `rgb()` requires all three the same;
    // `hsl()` and `hwb()` require a number or angle hue with both remaining
    // channels percentages.
    if legacy {
        let uniform = match channels[0] {
            Channel::Hue => {
                first_written == Written::Number
                    && second_written == Written::Percentage
                    && third_written == Written::Percentage
            },
            _ => {
                matches!(first_written, Written::Number | Written::Percentage)
                    && first_written == second_written
                    && second_written == third_written
            },
        };
        if !uniform {
            return Err(fail(input));
        }
    }

    let alpha = if legacy {
        if input.try_parse(|i| i.expect_comma()).is_ok() {
            parse_alpha(input, bindings)?
        } else {
            Some(1.0)
        }
    } else if input.try_parse(|i| i.expect_delim('/')).is_ok() {
        parse_alpha(input, bindings)?
    } else if let Some(bindings) = bindings {
        // An omitted alpha inherits the origin's, not 1.
        Some(relative::lookup(bindings, "alpha").unwrap_or(1.0))
    } else {
        Some(1.0)
    };

    // Legacy syntax has no missing components: `rgb(none, 0, 0)` is invalid.
    if legacy && (first.is_none() || second.is_none() || third.is_none() || alpha.is_none()) {
        return Err(fail(input));
    }

    Ok(Parsed {
        components: Components(
            first.unwrap_or(f32::NAN),
            second.unwrap_or(f32::NAN),
            third.unwrap_or(f32::NAN),
        ),
        alpha,
        legacy,
    })
}

/// Parse the body of one color function, given its lowercased name.
fn parse_color_function<'i>(
    input: &mut Parser<'i, '_>,
    name: &str,
) -> Result<Color, Failure<'i>> {
    if name.eq_ignore_ascii_case("color-mix") {
        return super::mix::parse_color_mix(input);
    }
    if name.eq_ignore_ascii_case("alpha") {
        return super::alpha::parse_alpha(input);
    }
    if name.eq_ignore_ascii_case("contrast-color") {
        return super::contrast::parse_contrast_color(input);
    }
    if name.eq_ignore_ascii_case("color-layers") {
        return super::layers::parse_color_layers(input);
    }
    if name.eq_ignore_ascii_case("color") {
        return parse_predefined(input);
    }

    let (space, channels) = match () {
        _ if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") => {
            (ColorSpace::Srgb, [Channel::Rgb8; 3])
        },
        _ if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") => (
            ColorSpace::Hsl,
            [Channel::Hue, Channel::Percent100, Channel::Percent100],
        ),
        _ if name.eq_ignore_ascii_case("hwb") => (
            ColorSpace::Hwb,
            [Channel::Hue, Channel::Percent100, Channel::Percent100],
        ),
        _ if name.eq_ignore_ascii_case("lab") => (
            ColorSpace::Lab,
            [Channel::LabLightness, Channel::LabAxis, Channel::LabAxis],
        ),
        _ if name.eq_ignore_ascii_case("lch") => (
            ColorSpace::Lch,
            [Channel::LabLightness, Channel::LabAxis, Channel::Hue],
        ),
        _ if name.eq_ignore_ascii_case("oklab") => (
            ColorSpace::Oklab,
            [Channel::OklabLightness, Channel::OklabAxis, Channel::OklabAxis],
        ),
        _ if name.eq_ignore_ascii_case("oklch") => (
            ColorSpace::Oklch,
            [Channel::OklabLightness, Channel::OklabAxis, Channel::Hue],
        ),
        _ => return Err(fail(input)),
    };

    let bindings = parse_origin(input, space, false)?;
    let relative = bindings.is_some();
    let parsed = parse_components(input, channels, bindings.as_ref())?;
    Ok(finish(space, parsed, relative))
}

/// The optional `from <color>` prefix of a relative color.
///
/// Returns `Ok(None)` for an ordinary absolute color. A `currentcolor`
/// origin is an error rather than a silent black: Livery has no cascade here
/// to resolve it against, and `color-mix()` rejects the same case.
fn parse_origin<'i>(
    input: &mut Parser<'i, '_>,
    space: ColorSpace,
    predefined: bool,
) -> Result<Option<Bindings>, Failure<'i>> {
    let found = input
        .try_parse(|i| i.expect_ident_matching("from"))
        .is_ok();
    if !found {
        return Ok(None);
    }
    let origin = parse_from(input)?;
    relative::bind(origin, space, predefined)
        .map(Some)
        .ok_or_else(|| fail(input))
}

/// `color([from <color>] <predefined-space> c1 c2 c3 [/ alpha])`.
///
/// The space follows the origin here, unlike every other function, so the
/// origin cannot be bound until it has been read.
fn parse_predefined<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    let relative_origin = if input
        .try_parse(|i| i.expect_ident_matching("from"))
        .is_ok()
    {
        Some(parse_from(input)?)
    } else {
        None
    };

    let location = input.current_source_location();
    let ident = input.expect_ident_cloned()?;
    let space = ColorSpace::from_predefined_name(&ident)
        .ok_or_else(|| location.new_custom_error(()))?;

    let bindings = match relative_origin {
        Some(origin) => Some(relative::bind(origin, space, true).ok_or_else(|| fail(input))?),
        None => None,
    };
    let parsed = parse_components(input, [Channel::Unit; 3], bindings.as_ref())?;
    if parsed.legacy {
        return Err(fail(input));
    }
    Ok(Color::Absolute {
        space,
        components: parsed.components,
        alpha: parsed.alpha.unwrap_or(f32::NAN),
        legacy: false,
    })
}

/// Apply per-space clamping and record the legacy serialization form.
fn finish(mut space: ColorSpace, parsed: Parsed, relative: bool) -> Color {
    let Parsed {
        mut components,
        alpha,
        legacy: _,
    } = parsed;

    match space {
        // Legacy rgb() clamps to the 0-255 device range and rounds; modern
        // rgb() keeps the float but still clamps, per CSS Color 4.
        ColorSpace::Srgb => {
            components = components.map(|v| {
                if v.is_nan() {
                    v
                } else {
                    (v / 255.0).clamp(0.0, 1.0)
                }
            });
        },
        ColorSpace::Hsl | ColorSpace::Hwb => {
            components = Components(
                if components.0.is_nan() {
                    f32::NAN
                } else {
                    normalize_hue(components.0)
                },
                clamp_non_negative(components.1),
                clamp_non_negative(components.2),
            );
        },
        ColorSpace::Lab | ColorSpace::Lch => {
            components.0 = clamp_range(components.0, 0.0, 100.0);
            if space == ColorSpace::Lch {
                components.1 = clamp_non_negative_unbounded(components.1);
                components.2 = if components.2.is_nan() {
                    f32::NAN
                } else {
                    normalize_hue(components.2)
                };
            }
        },
        ColorSpace::Oklab | ColorSpace::Oklch => {
            components.0 = clamp_range(components.0, 0.0, 1.0);
            if space == ColorSpace::Oklch {
                components.1 = clamp_non_negative_unbounded(components.1);
                components.2 = if components.2.is_nan() {
                    f32::NAN
                } else {
                    normalize_hue(components.2)
                };
            }
        },
        _ => {},
    }

    // Resolved relative rgb()/hsl()/hwb() values use modern color(srgb ...)
    // serialization. Missing hsl()/hwb() components are the exception: keep
    // that space so modern hsl()/hwb() can preserve their `none` identity.
    if relative
        && matches!(space, ColorSpace::Hsl | ColorSpace::Hwb)
        && !components.0.is_nan()
        && !components.1.is_nan()
        && !components.2.is_nan()
        && alpha.is_some()
    {
        components = space.convert(ColorSpace::Srgb, components);
        space = ColorSpace::Srgb;
    }
    let legacy = !relative && matches!(space, ColorSpace::Srgb | ColorSpace::Hsl | ColorSpace::Hwb);
    let alpha = alpha.unwrap_or(f32::NAN);
    Color::Absolute {
        space,
        components,
        alpha,
        legacy,
    }
}

fn clamp_range(value: f32, min: f32, max: f32) -> f32 {
    if value.is_nan() { value } else { value.clamp(min, max) }
}

fn clamp_non_negative(value: f32) -> f32 {
    if value.is_nan() { value } else { value.clamp(0.0, 100.0) }
}

fn clamp_non_negative_unbounded(value: f32) -> f32 {
    if value.is_nan() { value } else { value.max(0.0) }
}

/// Parse a whole color value from a string.
pub fn parse(source: &str) -> Result<Color, ParseError> {
    let mut buffer = ParserInput::new(source);
    let mut input = Parser::new(&mut buffer);
    let color = parse_from(&mut input).map_err(|_| ParseError::expected("a color"))?;
    input
        .expect_exhausted()
        .map_err(|_| ParseError::expected("a color"))?;
    Ok(color)
}

/// Parse one color from a parser already positioned at it. Shared with
/// `color-mix()`, which parses colors as arguments.
pub fn parse_from<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    let location = input.current_source_location();
    match input.next()?.clone() {
        Token::Hash(value) | Token::IDHash(value) => parse_hash(&value)
            .ok_or_else(|| location.new_custom_error(())),
        Token::Ident(name) => parse_keyword(&name)
            .ok_or_else(|| location.new_custom_error(())),
        Token::Function(name) => {
            input.parse_nested_block(|inner| {
                let color = parse_color_function(inner, &name)?;
                inner.expect_exhausted().map_err(|error| match error.kind {
                    BasicParseErrorKind::UnexpectedToken(_) => fail(inner),
                    _ => fail(inner),
                })?;
                Ok(color)
            })
        },
        _ => Err(location.new_custom_error(())),
    }
}

fn parse_hash(value: &str) -> Option<Color> {
    let (red, green, blue, alpha) = parse_hash_color(value.as_bytes()).ok()?;
    Some(Color::srgb8(red, green, blue, alpha))
}

fn parse_keyword(name: &str) -> Option<Color> {
    if name.eq_ignore_ascii_case("transparent") {
        return Some(Color::TRANSPARENT);
    }
    if name.eq_ignore_ascii_case("currentcolor") {
        return Some(Color::CurrentColor);
    }
    if let Some(system) = super::SystemColor::from_css_name(name) {
        return Some(Color::System(system));
    }
    let (red, green, blue) = parse_named_color(name).ok()?;
    Some(Color::srgb8(red, green, blue, 1.0))
}
