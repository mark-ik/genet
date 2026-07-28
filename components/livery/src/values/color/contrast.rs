/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Color 5's `contrast-color()` function.
//!
//! The level 5 algorithm is deliberately UA-defined. Livery compares black
//! and white using WCAG 2.1 relative luminance, after compositing a
//! translucent input over its current white Canvas palette color. This meets
//! the normative black-or-white result while keeping the policy explicit.
//!
//! <https://drafts.csswg.org/css-color-5/#contrast-color>

use cssparser::{ParseError as CssParseError, Parser};

use super::Color;

type Failure<'i> = CssParseError<'i, ()>;

pub fn parse_contrast_color<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    let origin = super::parse::parse_from(input)?;
    contrast_color(origin).ok_or_else(|| input.new_custom_error(()))
}

fn contrast_color(origin: Color) -> Option<Color> {
    let (red, green, blue, alpha) = origin.to_srgb()?;
    // A translucent background is made opaque against the current Canvas
    // palette color. Canvas is white in Livery's bounded palette today.
    let alpha = alpha.clamp(0.0, 1.0);
    let composite = |channel: f32| channel.clamp(0.0, 1.0) * alpha + 1.0 - alpha;
    let luminance = 0.2126 * linearize(composite(red))
        + 0.7152 * linearize(composite(green))
        + 0.0722 * linearize(composite(blue));
    let black_contrast = (luminance + 0.05) / 0.05;
    let white_contrast = 1.05 / (luminance + 0.05);
    if black_contrast > white_contrast {
        Some(Color::srgb8(0, 0, 0, 1.0))
    } else {
        // CSS Color 5 gives white the tie.
        Some(Color::srgb8(255, 255, 255, 1.0))
    }
}

fn linearize(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}
