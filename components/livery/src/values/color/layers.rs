/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Color 6's `color-layers()` function.
//!
//! Layers are listed topmost first. They are blended in sRGB and composited
//! with the Compositing and Blending Level 1 source-over formula.
//!
//! <https://drafts.csswg.org/css-color-6/#color-layers>
//! <https://drafts.fxtf.org/compositing-1/#blending>

use std::fmt;

use cssparser::{ParseError as CssParseError, Parser};

use super::space::Components;
use super::{Color, ColorSpace};

type Failure<'i> = CssParseError<'i, ()>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        Some(match () {
            _ if name.eq_ignore_ascii_case("normal") => Self::Normal,
            _ if name.eq_ignore_ascii_case("multiply") => Self::Multiply,
            _ if name.eq_ignore_ascii_case("screen") => Self::Screen,
            _ if name.eq_ignore_ascii_case("overlay") => Self::Overlay,
            _ if name.eq_ignore_ascii_case("darken") => Self::Darken,
            _ if name.eq_ignore_ascii_case("lighten") => Self::Lighten,
            _ if name.eq_ignore_ascii_case("color-dodge") => Self::ColorDodge,
            _ if name.eq_ignore_ascii_case("color-burn") => Self::ColorBurn,
            _ if name.eq_ignore_ascii_case("hard-light") => Self::HardLight,
            _ if name.eq_ignore_ascii_case("soft-light") => Self::SoftLight,
            _ if name.eq_ignore_ascii_case("difference") => Self::Difference,
            _ if name.eq_ignore_ascii_case("exclusion") => Self::Exclusion,
            _ if name.eq_ignore_ascii_case("hue") => Self::Hue,
            _ if name.eq_ignore_ascii_case("saturation") => Self::Saturation,
            _ if name.eq_ignore_ascii_case("color") => Self::Color,
            _ if name.eq_ignore_ascii_case("luminosity") => Self::Luminosity,
            _ => return None,
        })
    }

    pub(super) fn css_name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::ColorDodge => "color-dodge",
            Self::ColorBurn => "color-burn",
            Self::HardLight => "hard-light",
            Self::SoftLight => "soft-light",
            Self::Difference => "difference",
            Self::Exclusion => "exclusion",
            Self::Hue => "hue",
            Self::Saturation => "saturation",
            Self::Color => "color",
            Self::Luminosity => "luminosity",
        }
    }
}

impl fmt::Display for BlendMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.css_name())
    }
}

pub fn parse_color_layers<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    let mode = input
        .try_parse(|nested| {
            let location = nested.current_source_location();
            let name = nested.expect_ident_cloned()?;
            let mode = BlendMode::from_name(&name).ok_or_else(|| location.new_custom_error(()))?;
            nested.expect_comma()?;
            Ok::<_, Failure<'i>>(mode)
        })
        .unwrap_or(BlendMode::Normal);

    let mut colors = vec![super::parse::parse_from(input)?];
    while input.try_parse(|nested| nested.expect_comma()).is_ok() {
        colors.push(super::parse::parse_from(input)?);
    }
    resolve_layers(mode, &colors).ok_or_else(|| input.new_custom_error(()))
}

fn resolve_layers(mode: BlendMode, colors: &[Color]) -> Option<Color> {
    let (last, rest) = colors.split_last()?;
    let mut backdrop = rgba(*last)?;
    for source in rest.iter().rev() {
        backdrop = composite(mode, rgba(*source)?, backdrop);
    }
    Some(Color::Absolute {
        space: ColorSpace::Srgb,
        components: Components(backdrop[0], backdrop[1], backdrop[2]),
        alpha: backdrop[3],
        legacy: false,
    })
}

fn rgba(color: Color) -> Option<[f32; 4]> {
    let (red, green, blue, alpha) = color.to_srgb()?;
    Some([
        red.clamp(0.0, 1.0),
        green.clamp(0.0, 1.0),
        blue.clamp(0.0, 1.0),
        alpha.clamp(0.0, 1.0),
    ])
}

/// Blend `source` over `backdrop`; both arrays carry unpremultiplied sRGB.
fn composite(mode: BlendMode, source: [f32; 4], backdrop: [f32; 4]) -> [f32; 4] {
    let source_alpha = source[3];
    let backdrop_alpha = backdrop[3];
    let alpha = source_alpha + backdrop_alpha * (1.0 - source_alpha);
    if alpha == 0.0 {
        return [0.0; 4];
    }

    let blended = blend(
        mode,
        [backdrop[0], backdrop[1], backdrop[2]],
        [source[0], source[1], source[2]],
    );
    let mut result = [0.0; 4];
    for index in 0..3 {
        // Compositing and Blending 1 §10: blend in place, then source-over.
        let source_prime = (1.0 - backdrop_alpha) * source[index] + backdrop_alpha * blended[index];
        let premultiplied =
            source_alpha * source_prime + backdrop_alpha * (1.0 - source_alpha) * backdrop[index];
        result[index] = (premultiplied / alpha).clamp(0.0, 1.0);
    }
    result[3] = alpha;
    result
}

fn blend(mode: BlendMode, backdrop: [f32; 3], source: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(source, sat(backdrop)), lum(backdrop)),
        BlendMode::Saturation => set_lum(set_sat(backdrop, sat(source)), lum(backdrop)),
        BlendMode::Color => set_lum(source, lum(backdrop)),
        BlendMode::Luminosity => set_lum(backdrop, lum(source)),
        _ => [
            blend_channel(mode, backdrop[0], source[0]),
            blend_channel(mode, backdrop[1], source[1]),
            blend_channel(mode, backdrop[2], source[2]),
        ],
    }
}

fn blend_channel(mode: BlendMode, backdrop: f32, source: f32) -> f32 {
    match mode {
        BlendMode::Normal => source,
        BlendMode::Multiply => backdrop * source,
        BlendMode::Screen => backdrop + source - backdrop * source,
        BlendMode::Overlay => hard_light(source, backdrop),
        BlendMode::Darken => backdrop.min(source),
        BlendMode::Lighten => backdrop.max(source),
        BlendMode::ColorDodge if backdrop == 0.0 => 0.0,
        BlendMode::ColorDodge if source >= 1.0 => 1.0,
        BlendMode::ColorDodge => (backdrop / (1.0 - source)).min(1.0),
        BlendMode::ColorBurn if backdrop >= 1.0 => 1.0,
        BlendMode::ColorBurn if source == 0.0 => 0.0,
        BlendMode::ColorBurn => 1.0 - ((1.0 - backdrop) / source).min(1.0),
        BlendMode::HardLight => hard_light(backdrop, source),
        BlendMode::SoftLight if source <= 0.5 => {
            backdrop - (1.0 - 2.0 * source) * backdrop * (1.0 - backdrop)
        },
        BlendMode::SoftLight => {
            let d = if backdrop <= 0.25 {
                ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop
            } else {
                backdrop.sqrt()
            };
            backdrop + (2.0 * source - 1.0) * (d - backdrop)
        },
        BlendMode::Difference => (backdrop - source).abs(),
        BlendMode::Exclusion => backdrop + source - 2.0 * backdrop * source,
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => {
            unreachable!("non-separable modes are handled as vectors")
        },
    }
}

fn hard_light(backdrop: f32, source: f32) -> f32 {
    if source <= 0.5 {
        backdrop * 2.0 * source
    } else {
        backdrop + (2.0 * source - 1.0) - backdrop * (2.0 * source - 1.0)
    }
}

fn lum(color: [f32; 3]) -> f32 {
    0.3 * color[0] + 0.59 * color[1] + 0.11 * color[2]
}

fn sat(color: [f32; 3]) -> f32 {
    color.into_iter().fold(f32::MIN, f32::max) - color.into_iter().fold(f32::MAX, f32::min)
}

fn set_lum(mut color: [f32; 3], target: f32) -> [f32; 3] {
    let delta = target - lum(color);
    for channel in &mut color {
        *channel += delta;
    }
    clip_color(color)
}

fn clip_color(mut color: [f32; 3]) -> [f32; 3] {
    let luminance = lum(color);
    let minimum = color.into_iter().fold(f32::MAX, f32::min);
    let maximum = color.into_iter().fold(f32::MIN, f32::max);
    if minimum < 0.0 {
        for channel in &mut color {
            *channel = luminance + ((*channel - luminance) * luminance) / (luminance - minimum);
        }
    }
    if maximum > 1.0 {
        for channel in &mut color {
            *channel =
                luminance + ((*channel - luminance) * (1.0 - luminance)) / (maximum - luminance);
        }
    }
    color
}

fn set_sat(mut color: [f32; 3], target: f32) -> [f32; 3] {
    let mut indices = [0, 1, 2];
    indices.sort_by(|left, right| color[*left].total_cmp(&color[*right]));
    let [minimum, middle, maximum] = indices;
    if color[maximum] > color[minimum] {
        color[middle] =
            ((color[middle] - color[minimum]) * target) / (color[maximum] - color[minimum]);
        color[maximum] = target;
    } else {
        color[middle] = 0.0;
        color[maximum] = 0.0;
    }
    color[minimum] = 0.0;
    color
}
