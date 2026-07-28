/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `color-mix()` and the interpolation it rides on.
//!
//! Harvest lift (cutover plan F0), shaped after the stylo fork's
//! `style/color/mix.rs` at `b157d925267fdd37b03f43e3387ab2f0909e57b0`.
//! Implements CSS Color 5 <https://drafts.csswg.org/css-color-5/#color-mix>.

use cssparser::{ParseError as CssParseError, Parser};

use super::Color;
use super::space::{ColorSpace, Components, normalize_hue};

type Failure<'i> = CssParseError<'i, ()>;

/// How a hue channel takes the short or long way round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HueInterpolation {
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

impl HueInterpolation {
    fn from_name(name: &str) -> Option<Self> {
        Some(match () {
            _ if name.eq_ignore_ascii_case("shorter") => Self::Shorter,
            _ if name.eq_ignore_ascii_case("longer") => Self::Longer,
            _ if name.eq_ignore_ascii_case("increasing") => Self::Increasing,
            _ if name.eq_ignore_ascii_case("decreasing") => Self::Decreasing,
            _ => return None,
        })
    }

    /// Adjust a hue pair before linear interpolation.
    /// <https://drafts.csswg.org/css-color-4/#hue-interpolation>
    fn adjust(self, mut left: f32, mut right: f32) -> (f32, f32) {
        left = normalize_hue(left);
        right = normalize_hue(right);
        let difference = right - left;
        match self {
            Self::Shorter => {
                if difference > 180.0 {
                    left += 360.0;
                } else if difference < -180.0 {
                    right += 360.0;
                }
            },
            Self::Longer => {
                if (0.0..=180.0).contains(&difference) {
                    left += 360.0;
                } else if (-180.0..=0.0).contains(&difference) {
                    right += 360.0;
                }
            },
            Self::Increasing => {
                if difference < 0.0 {
                    right += 360.0;
                }
            },
            Self::Decreasing => {
                if difference > 0.0 {
                    left += 360.0;
                }
            },
        }
        (left, right)
    }
}

/// `color-mix([in <space> [<hue> hue]?,]? [<color> && <percentage>?]#)`
pub fn parse_color_mix<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    let interpolation = input
        .try_parse(|nested| {
            nested.expect_ident_matching("in")?;
            let location = nested.current_source_location();
            let space_name = nested.expect_ident_cloned()?;
            let space = ColorSpace::from_interpolation_name(&space_name)
                .ok_or_else(|| location.new_custom_error(()))?;

            // The hue interpolation method is only allowed for polar spaces.
            let hue_method = if space.hue_index().is_some() {
                nested
                    .try_parse(|i| {
                        let name = i.expect_ident_cloned()?;
                        let method = HueInterpolation::from_name(&name)
                            .ok_or_else(|| i.new_custom_error::<(), ()>(()))?;
                        i.expect_ident_matching("hue")?;
                        Ok::<_, Failure<'i>>(method)
                    })
                    .unwrap_or(HueInterpolation::Shorter)
            } else {
                HueInterpolation::Shorter
            };
            nested.expect_comma()?;
            Ok::<_, Failure<'i>>((space, hue_method))
        })
        .ok();
    let (space, hue_method) =
        interpolation.unwrap_or((ColorSpace::Oklab, HueInterpolation::Shorter));

    let mut items = vec![parse_mix_operand(input)?];
    while input.try_parse(|nested| nested.expect_comma()).is_ok() {
        items.push(parse_mix_operand(input)?);
    }
    mix_many(space, hue_method, &items).ok_or_else(|| input.new_custom_error(()))
}

/// A color with an optional percentage, in either order.
fn parse_mix_operand<'i>(input: &mut Parser<'i, '_>) -> Result<(Color, Option<f32>), Failure<'i>> {
    let leading = input.try_parse(parse_mix_percentage).ok();
    let color = super::parse::parse_from(input)?;
    let percentage = match leading {
        Some(value) => Some(value),
        None => input.try_parse(parse_mix_percentage).ok(),
    };
    if percentage.is_some_and(|value| !(0.0..=100.0).contains(&value)) {
        return Err(input.new_custom_error(()));
    }
    Ok((color, percentage))
}

fn parse_mix_percentage<'i>(input: &mut Parser<'i, '_>) -> Result<f32, Failure<'i>> {
    let start = input.state();
    if let Ok(value) = input.expect_percentage() {
        return Ok(value * 100.0);
    }
    input.reset(&start);
    let position = input.position();
    let name = input.expect_function()?.clone();
    if !super::parse::is_math_function(&name) {
        return Err(input.new_custom_error(()));
    }
    input.parse_nested_block(|nested| {
        while nested.next().is_ok() {}
        Ok::<_, Failure<'i>>(())
    })?;
    crate::values::calc::parse_percentage(input.slice_from(position), 100.0)
        .map_err(|_| input.new_custom_error(()))
}

/// Mix two colors in `space`.
pub fn mix(
    space: ColorSpace,
    hue_method: HueInterpolation,
    left: &Color,
    left_percentage: Option<f32>,
    right: &Color,
    right_percentage: Option<f32>,
) -> Option<Color> {
    mix_many(
        space,
        hue_method,
        &[(*left, left_percentage), (*right, right_percentage)],
    )
}

/// Resolve the current one-or-more-item CSS Color 5 mixing algorithm.
pub(super) fn mix_many(
    space: ColorSpace,
    hue_method: HueInterpolation,
    items: &[(Color, Option<f32>)],
) -> Option<Color> {
    let (weights, alpha_multiplier) = normalize_percentages(items)?;
    if weights.iter().all(|weight| *weight == 0.0) {
        let mut components = Components(0.0, 0.0, 0.0);
        if let Some(hue) = space.hue_index() {
            match hue {
                0 => components.0 = f32::NAN,
                1 => components.1 = f32::NAN,
                _ => components.2 = f32::NAN,
            }
        }
        return Some(Color::Absolute {
            space,
            components,
            alpha: 0.0,
            legacy: false,
        });
    }

    let mut accumulated = convert_to_space(items.first()?.0, space)?;
    let mut combined_weight = weights[0];
    for ((color, _), weight) in items.iter().skip(1).zip(weights.iter().skip(1)) {
        let total = combined_weight + *weight;
        let progress = if total == 0.0 { 0.5 } else { *weight / total };
        accumulated = mix_pair(
            space,
            hue_method,
            accumulated,
            1.0 - progress,
            convert_to_space(*color, space)?,
            progress,
        );
        combined_weight = total;
    }
    Some(multiply_alpha(accumulated, alpha_multiplier))
}

fn normalize_percentages(items: &[(Color, Option<f32>)]) -> Option<(Vec<f32>, f32)> {
    if items.is_empty() {
        return None;
    }
    let specified_sum = items
        .iter()
        .filter_map(|(_, percentage)| *percentage)
        .sum::<f32>();
    let omitted = items
        .iter()
        .filter(|(_, percentage)| percentage.is_none())
        .count();
    if !specified_sum.is_finite()
        || items.iter().any(|(_, percentage)| {
            percentage.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
        })
    {
        return None;
    }
    let omitted_weight = if omitted == 0 {
        0.0
    } else {
        (100.0 - specified_sum.min(100.0)) / omitted as f32
    };
    let mut weights = items
        .iter()
        .map(|(_, percentage)| percentage.unwrap_or(omitted_weight))
        .collect::<Vec<_>>();
    let total = weights.iter().sum::<f32>();
    let alpha_multiplier = if total < 100.0 { total / 100.0 } else { 1.0 };
    if total > 0.0 {
        for weight in &mut weights {
            *weight /= total;
        }
    }
    Some((weights, alpha_multiplier))
}

fn convert_to_space(color: Color, space: ColorSpace) -> Option<Color> {
    let (components, alpha) = color.to_space(space)?;
    Some(Color::Absolute {
        space,
        components,
        alpha,
        legacy: false,
    })
}

fn mix_pair(
    space: ColorSpace,
    hue_method: HueInterpolation,
    left: Color,
    left_weight: f32,
    right: Color,
    right_weight: f32,
) -> Color {
    let (left_components, left_alpha) = left
        .to_space(space)
        .expect("pair was converted to mixing space");
    let (right_components, right_alpha) = right
        .to_space(space)
        .expect("pair was converted to mixing space");

    // Premultiply by alpha before interpolating, except for missing channels.
    let left_alpha_resolved = if left_alpha.is_nan() { 1.0 } else { left_alpha };
    let right_alpha_resolved = if right_alpha.is_nan() {
        1.0
    } else {
        right_alpha
    };

    let hue_index = space.hue_index();
    let mut out = [0.0f32; 3];
    for (index, output) in out.iter_mut().enumerate() {
        let left_value = component(left_components, index);
        let right_value = component(right_components, index);

        // A channel missing on one side takes the other side's value.
        let (left_value, right_value) = match (left_value.is_nan(), right_value.is_nan()) {
            (true, true) => {
                *output = f32::NAN;
                continue;
            },
            (true, false) => (right_value, right_value),
            (false, true) => (left_value, left_value),
            (false, false) => (left_value, right_value),
        };

        if hue_index == Some(index) {
            let (left_hue, right_hue) = hue_method.adjust(left_value, right_value);
            *output = normalize_hue(left_hue * left_weight + right_hue * right_weight);
        } else {
            // Premultiplied interpolation: scale by alpha, mix, then unscale.
            let mixed_alpha =
                left_alpha_resolved * left_weight + right_alpha_resolved * right_weight;
            let premultiplied = left_value * left_alpha_resolved * left_weight
                + right_value * right_alpha_resolved * right_weight;
            *output = if mixed_alpha == 0.0 {
                0.0
            } else {
                premultiplied / mixed_alpha
            };
        }
    }

    let alpha = if left_alpha.is_nan() && right_alpha.is_nan() {
        f32::NAN
    } else {
        left_alpha_resolved * left_weight + right_alpha_resolved * right_weight
    };

    Color::Absolute {
        space,
        components: Components(out[0], out[1], out[2]),
        alpha: if alpha.is_nan() {
            alpha
        } else {
            alpha.clamp(0.0, 1.0)
        },
        // A mix result is a modern value: `color-mix()` serializes in the
        // interpolation space, never as legacy rgb().
        legacy: false,
    }
}

fn multiply_alpha(color: Color, multiplier: f32) -> Color {
    match color {
        Color::Absolute {
            space,
            components,
            alpha,
            legacy,
        } => Color::Absolute {
            space,
            components,
            alpha: if alpha.is_nan() {
                alpha
            } else {
                (alpha * multiplier).clamp(0.0, 1.0)
            },
            legacy,
        },
        other => other,
    }
}

fn component(components: Components, index: usize) -> f32 {
    match index {
        0 => components.0,
        1 => components.1,
        _ => components.2,
    }
}
