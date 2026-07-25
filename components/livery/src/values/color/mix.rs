/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `color-mix()` and the interpolation it rides on.
//!
//! Harvest lift (cutover plan F0), shaped after the stylo fork's
//! `style/color/mix.rs` at `b157d925267fdd37b03f43e3387ab2f0909e57b0`.
//! Implements CSS Color 5 <https://drafts.csswg.org/css-color-5/#color-mix>.

use cssparser::{ParseError as CssParseError, Parser};

use super::space::{ColorSpace, Components, normalize_hue};
use super::Color;

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

/// `color-mix(in <space> [<hue> hue]?, <color> [<percentage>]?, <color> [<percentage>]?)`
pub fn parse_color_mix<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    input.expect_ident_matching("in")?;

    let location = input.current_source_location();
    let space_name = input.expect_ident_cloned()?;
    let space = ColorSpace::from_interpolation_name(&space_name)
        .ok_or_else(|| location.new_custom_error(()))?;

    // The hue interpolation method is only allowed for polar spaces.
    let hue_method = if space.hue_index().is_some() {
        input
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

    input.expect_comma()?;
    let (first, first_percentage) = parse_mix_operand(input)?;
    input.expect_comma()?;
    let (second, second_percentage) = parse_mix_operand(input)?;

    mix(
        space,
        hue_method,
        &first,
        first_percentage,
        &second,
        second_percentage,
    )
    .ok_or_else(|| input.new_custom_error(()))
}

/// A color with an optional percentage, in either order.
fn parse_mix_operand<'i>(
    input: &mut Parser<'i, '_>,
) -> Result<(Color, Option<f32>), Failure<'i>> {
    let leading = input
        .try_parse(|i| i.expect_percentage())
        .ok()
        .map(|value| value * 100.0);
    let color = super::parse::parse_from(input)?;
    let percentage = match leading {
        Some(value) => Some(value),
        None => input
            .try_parse(|i| i.expect_percentage())
            .ok()
            .map(|value| value * 100.0),
    };
    if percentage.is_some_and(|value| !(0.0..=100.0).contains(&value)) {
        return Err(input.new_custom_error(()));
    }
    Ok((color, percentage))
}

/// Mix two colors in `space`. Returns `None` when both percentages are zero,
/// which CSS Color 5 makes invalid.
pub fn mix(
    space: ColorSpace,
    hue_method: HueInterpolation,
    left: &Color,
    left_percentage: Option<f32>,
    right: &Color,
    right_percentage: Option<f32>,
) -> Option<Color> {
    // Percentage normalization, per the spec: omitted percentages split the
    // remainder; a total below 100% scales alpha rather than the components.
    let (mut left_weight, mut right_weight, alpha_multiplier) =
        match (left_percentage, right_percentage) {
            (None, None) => (0.5, 0.5, 1.0),
            (Some(left), None) => (left / 100.0, 1.0 - left / 100.0, 1.0),
            (None, Some(right)) => (1.0 - right / 100.0, right / 100.0, 1.0),
            (Some(left), Some(right)) => {
                let sum = left + right;
                if sum <= 0.0 {
                    return None;
                }
                let multiplier = if sum < 100.0 { sum / 100.0 } else { 1.0 };
                (left / sum, right / sum, multiplier)
            },
        };
    if !left_weight.is_finite() || !right_weight.is_finite() {
        return None;
    }
    // `currentcolor` has no components until the cascade runs, so a mix
    // involving it cannot be resolved here and fails rather than silently
    // becoming black. System colors do resolve, through Livery's bounded
    // palette.
    let left_absolute = left.to_space(space)?;
    let right_absolute = right.to_space(space)?;

    if left_weight <= 0.0 && right_weight <= 0.0 {
        return None;
    }
    left_weight = left_weight.clamp(0.0, 1.0);
    right_weight = right_weight.clamp(0.0, 1.0);

    let (left_components, left_alpha) = left_absolute;
    let (right_components, right_alpha) = right_absolute;

    // Premultiply by alpha before interpolating, except for missing channels.
    let left_alpha_resolved = if left_alpha.is_nan() { 1.0 } else { left_alpha };
    let right_alpha_resolved = if right_alpha.is_nan() { 1.0 } else { right_alpha };

    let hue_index = space.hue_index();
    let mut out = [0.0f32; 3];
    for index in 0..3 {
        let left_value = component(left_components, index);
        let right_value = component(right_components, index);

        // A channel missing on one side takes the other side's value.
        let (left_value, right_value) = match (left_value.is_nan(), right_value.is_nan()) {
            (true, true) => {
                out[index] = f32::NAN;
                continue;
            },
            (true, false) => (right_value, right_value),
            (false, true) => (left_value, left_value),
            (false, false) => (left_value, right_value),
        };

        if hue_index == Some(index) {
            let (left_hue, right_hue) = hue_method.adjust(left_value, right_value);
            out[index] = normalize_hue(left_hue * left_weight + right_hue * right_weight);
        } else {
            // Premultiplied interpolation: scale by alpha, mix, then unscale.
            let mixed_alpha =
                left_alpha_resolved * left_weight + right_alpha_resolved * right_weight;
            let premultiplied = left_value * left_alpha_resolved * left_weight
                + right_value * right_alpha_resolved * right_weight;
            out[index] = if mixed_alpha == 0.0 {
                0.0
            } else {
                premultiplied / mixed_alpha
            };
        }
    }

    let alpha = if left_alpha.is_nan() && right_alpha.is_nan() {
        f32::NAN
    } else {
        (left_alpha_resolved * left_weight + right_alpha_resolved * right_weight)
            * alpha_multiplier
    };

    Some(Color::Absolute {
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
    })
}

fn component(components: Components, index: usize) -> f32 {
    match index {
        0 => components.0,
        1 => components.1,
        _ => components.2,
    }
}
