/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS colors.
//!
//! Cutover plan F0's first slice. Livery previously parsed hex, named colors,
//! and `rgb()`/`rgba()` only; this carries the CSS Color 4 model (absolute
//! colors in any of fourteen spaces, missing components, float channels) plus
//! CSS Color 5's `color-mix()`.
//!
//! Provenance: the conversion math and the mixing rules are lifted from the
//! stylo fork under the harvest plan's fork-and-own rules. Per-module headers
//! name what came from where.

mod mix;
mod parse;
mod relative;
mod space;

use std::{fmt, str::FromStr};

pub use mix::HueInterpolation;
pub use space::ColorSpace;

use space::Components;

use super::{ParseError, format_number};

/// A system color, whose used value depends on the device palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemColor {
    CanvasText,
    Canvas,
    LinkText,
    VisitedText,
    ActiveText,
    ButtonFace,
    ButtonText,
    ButtonBorder,
    Field,
    FieldText,
    Highlight,
    HighlightText,
    Mark,
    MarkText,
    GrayText,
    AccentColor,
    AccentColorText,
    SelectedItem,
    SelectedItemText,
}

impl SystemColor {
    const TABLE: &'static [(&'static str, Self)] = &[
        ("canvastext", Self::CanvasText),
        ("canvas", Self::Canvas),
        ("linktext", Self::LinkText),
        ("visitedtext", Self::VisitedText),
        ("activetext", Self::ActiveText),
        ("buttonface", Self::ButtonFace),
        ("buttontext", Self::ButtonText),
        ("buttonborder", Self::ButtonBorder),
        ("field", Self::Field),
        ("fieldtext", Self::FieldText),
        ("highlight", Self::Highlight),
        ("highlighttext", Self::HighlightText),
        ("mark", Self::Mark),
        ("marktext", Self::MarkText),
        ("graytext", Self::GrayText),
        ("accentcolor", Self::AccentColor),
        ("accentcolortext", Self::AccentColorText),
        ("selecteditem", Self::SelectedItem),
        ("selecteditemtext", Self::SelectedItemText),
    ];

    fn from_css_name(name: &str) -> Option<Self> {
        Self::TABLE
            .iter()
            .find(|(key, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| *value)
    }

    /// The CSS-cased name, as serialization requires.
    pub fn css_name(self) -> &'static str {
        match self {
            Self::CanvasText => "canvastext",
            Self::Canvas => "canvas",
            Self::LinkText => "linktext",
            Self::VisitedText => "visitedtext",
            Self::ActiveText => "activetext",
            Self::ButtonFace => "buttonface",
            Self::ButtonText => "buttontext",
            Self::ButtonBorder => "buttonborder",
            Self::Field => "field",
            Self::FieldText => "fieldtext",
            Self::Highlight => "highlight",
            Self::HighlightText => "highlighttext",
            Self::Mark => "mark",
            Self::MarkText => "marktext",
            Self::GrayText => "graytext",
            Self::AccentColor => "accentcolor",
            Self::AccentColorText => "accentcolortext",
            Self::SelectedItem => "selecteditem",
            Self::SelectedItemText => "selecteditemtext",
        }
    }

    /// The palette value Livery paints with today. Kept deliberately small:
    /// a real device palette arrives with the theming lane.
    fn used_srgb(self) -> (f32, f32, f32) {
        match self {
            Self::Canvas | Self::ButtonFace | Self::Field => (1.0, 1.0, 1.0),
            Self::LinkText => (0.0, 0.0, 0.933_333_3),
            Self::VisitedText => (0.333_333_3, 0.101_960_78, 0.545_098),
            Self::ActiveText => (1.0, 0.0, 0.0),
            Self::GrayText => (0.4, 0.4, 0.4),
            Self::Highlight | Self::SelectedItem | Self::AccentColor => {
                (0.129_411_77, 0.376_470_6, 0.803_921_6)
            },
            Self::HighlightText | Self::SelectedItemText | Self::AccentColorText => {
                (1.0, 1.0, 1.0)
            },
            Self::Mark => (1.0, 1.0, 0.0),
            _ => (0.0, 0.0, 0.0),
        }
    }
}

/// A CSS color.
///
/// `Absolute` carries components in their authored space so serialization can
/// round-trip; `CurrentColor` and `System` stay unresolved until used-value
/// time, when the cascade knows the inherited color and the device palette.
///
/// Component channels may be NaN, which encodes CSS `none` (a missing
/// component). Every accessor that hands a number to a consumer resolves it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    CurrentColor,
    System(SystemColor),
    Absolute {
        space: ColorSpace,
        components: Components,
        alpha: f32,
        /// Whether this serializes in the legacy `rgb()`/`rgba()` form.
        ///
        /// True for `rgb()`, `rgba()`, hex, named colors, `hsl()`, `hsla()`,
        /// and `hwb()`, whichever syntax authored them; false for `color()`
        /// and the Lab/Oklab family. It cannot be derived from `space`,
        /// because `rgb(0 0 0)` and `color(srgb 0 0 0)` share sRGB and
        /// serialize differently.
        legacy: bool,
    },
}

impl Color {
    /// `transparent`, which CSS Color 4 defines as `rgba(0, 0, 0, 0)`.
    pub const TRANSPARENT: Self = Self::Absolute {
        space: ColorSpace::Srgb,
        components: Components(0.0, 0.0, 0.0),
        alpha: 0.0,
        legacy: true,
    };

    /// The `CanvasText` system color, Livery's initial `color`.
    pub const CANVAS_TEXT: Self = Self::System(SystemColor::CanvasText);

    /// An sRGB color from 8-bit channels and a float alpha.
    pub fn srgb8(red: u8, green: u8, blue: u8, alpha: f32) -> Self {
        Self::Absolute {
            space: ColorSpace::Srgb,
            components: Components(
                f32::from(red) / 255.0,
                f32::from(green) / 255.0,
                f32::from(blue) / 255.0,
            ),
            alpha: quantize_alpha(alpha),
            legacy: true,
        }
    }

    /// An sRGB color from float channels in 0-1.
    pub fn srgb(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self::Absolute {
            space: ColorSpace::Srgb,
            components: Components(red, green, blue),
            alpha: quantize_alpha(alpha),
            legacy: true,
        }
    }

    /// Whether this color needs the cascade before it has components.
    pub fn is_unresolved(self) -> bool {
        matches!(self, Self::CurrentColor | Self::System(_))
    }

    /// This color's components in `space`, or `None` if it is unresolved.
    pub(crate) fn components_in(self, space: ColorSpace) -> Option<(Components, f32)> {
        self.to_space(space)
    }

    fn to_space(self, space: ColorSpace) -> Option<(Components, f32)> {
        match self {
            Self::CurrentColor => None,
            Self::System(system) => {
                let (red, green, blue) = system.used_srgb();
                Some((
                    ColorSpace::Srgb.convert(space, Components(red, green, blue)),
                    1.0,
                ))
            },
            Self::Absolute {
                space: from,
                components,
                alpha,
                ..
            } => Some((from.convert(space, components), alpha)),
        }
    }

    /// Resolved sRGB channels in 0-1, with `none` resolved to zero.
    ///
    /// `None` for `currentcolor` and system colors, which the caller must
    /// resolve first. This is the accessor paint uses.
    pub fn to_srgb(self) -> Option<(f32, f32, f32, f32)> {
        let (components, alpha) = self.to_space(ColorSpace::Srgb)?;
        let Components(red, green, blue) = components.resolve_missing();
        Some((red, green, blue, if alpha.is_nan() { 0.0 } else { alpha }))
    }

    /// Resolved, gamut-clipped 8-bit sRGB channels.
    ///
    /// Clipping rather than gamut mapping: a wide-gamut color outside sRGB is
    /// clamped per channel. Proper gamut mapping (CSS Color 4's oklch chroma
    /// reduction) is a named follow-on, not silently pretended here.
    pub fn to_srgb8(self) -> Option<(u8, u8, u8, u8)> {
        let (red, green, blue, alpha) = self.to_srgb()?;
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some((
            channel(red),
            channel(green),
            channel(blue),
            channel(alpha),
        ))
    }

    /// Interpolate two colors for the animation clock.
    ///
    /// Unresolved endpoints stay discrete: their used value depends on the
    /// cascade, so there is nothing to interpolate at this layer.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        if progress <= 0.0 {
            return self;
        }
        if progress >= 1.0 {
            return other;
        }
        if self.is_unresolved() || other.is_unresolved() {
            return self;
        }
        // CSS Color 4 interpolates in oklab by default, but property
        // animation of `<color>` stays in sRGB for compatibility with the
        // legacy behavior every engine ships.
        mix::mix(
            ColorSpace::Srgb,
            HueInterpolation::Shorter,
            &self,
            Some(f64::from(1.0 - progress) as f32 * 100.0),
            &other,
            Some(progress * 100.0),
        )
        // An interpolated color keeps the endpoints' serialization form: two
        // legacy colors must not start serializing as `color(srgb ...)`
        // partway through an animation.
        .map(|mixed| match (self.is_legacy(), other.is_legacy()) {
            (true, true) => mixed.as_legacy(),
            _ => mixed,
        })
        .unwrap_or(self)
    }

    fn is_legacy(self) -> bool {
        matches!(self, Self::Absolute { legacy: true, .. })
    }

    fn as_legacy(self) -> Self {
        match self {
            Self::Absolute {
                space,
                components,
                alpha,
                ..
            } => Self::Absolute {
                space,
                components,
                alpha: quantize_alpha(alpha),
                legacy: true,
            },
            other => other,
        }
    }

    /// Mix two colors, the engine-facing half of `color-mix()`.
    pub fn mix(
        space: ColorSpace,
        hue: HueInterpolation,
        left: Self,
        left_percentage: Option<f32>,
        right: Self,
        right_percentage: Option<f32>,
    ) -> Option<Self> {
        mix::mix(space, hue, &left, left_percentage, &right, right_percentage)
    }
}

impl FromStr for Color {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        parse::parse(input.trim())
    }
}

/// Serialize one channel, rendering a missing component as `none`.
/// Serialize one channel, rendering a missing component as `none`.
///
/// `f32::to_string` already yields the shortest decimal that round-trips to
/// the same `f32`, so no rounding is applied here. Rounding would break
/// serialize-then-reparse for a channel like `0.123456789`.
fn channel(value: f32) -> String {
    if value.is_nan() {
        "none".to_owned()
    } else {
        format_number(value)
    }
}

/// The alpha of a non-legacy color: `/ <number>`, omitted when fully opaque.
fn alpha_suffix(alpha: f32) -> String {
    if alpha.is_nan() {
        return " / none".to_owned();
    }
    if alpha >= 1.0 {
        return String::new();
    }
    format!(" / {}", format_number(alpha))
}

/// Snap alpha to the 8-bit grid the legacy form serializes on.
///
/// The legacy `rgb()`/`rgba()` form is an 8-bit value end to end, so its alpha
/// is quantized on the way in. Without this, `#33669980` (alpha 128/255)
/// serializes as `0.502` and reparses as a different float, and
/// serialize-then-reparse stops being stable.
pub(crate) fn quantize_alpha(alpha: f32) -> f32 {
    if alpha.is_nan() {
        return alpha;
    }
    (alpha.clamp(0.0, 1.0) * 255.0).round() / 255.0
}

/// Legacy alpha serialization: three decimals, trailing zeros trimmed.
///
/// Three decimals is exactly enough to distinguish every 8-bit alpha (1/255 is
/// about 0.0039), and it is the shape other engines emit, so `#33669980`
/// serializes as `rgba(51, 102, 153, 0.502)`. Paired with `quantize_alpha`,
/// reparsing lands on the same float.
fn legacy_alpha(alpha: f32) -> String {
    let text = format!("{:.3}", alpha);
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() }
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::CurrentColor => formatter.write_str("currentcolor"),
            Self::System(system) => formatter.write_str(system.css_name()),
            Self::Absolute {
                space,
                components,
                alpha,
                legacy,
            } => serialize_absolute(formatter, space, components, alpha, legacy),
        }
    }
}

/// CSS Color 4 resolves the whole sRGB family (`rgb()`, `hsl()`, `hwb()`, hex,
/// named) to the legacy `rgb()`/`rgba()` form; every other space serializes in
/// its own function. The authored space is kept on the value because mixing
/// and `none` need it, but it does not survive into serialization for that
/// family. This is the `getComputedStyle` shape.
fn serialize_absolute(
    formatter: &mut fmt::Formatter<'_>,
    space: ColorSpace,
    components: Components,
    alpha: f32,
    legacy: bool,
) -> fmt::Result {
    if legacy {
        let Components(red, green, blue) = space
            .convert(ColorSpace::Srgb, components)
            .resolve_missing();
        let to8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        let alpha = if alpha.is_nan() { 0.0 } else { alpha };
        return if alpha >= 1.0 {
            write!(formatter, "rgb({}, {}, {})", to8(red), to8(green), to8(blue))
        } else {
            write!(
                formatter,
                "rgba({}, {}, {}, {})",
                to8(red),
                to8(green),
                to8(blue),
                legacy_alpha(alpha)
            )
        };
    }

    let Components(first, second, third) = components;
    if let Some(name) = space.predefined_name() {
        return write!(
            formatter,
            "color({name} {} {} {}{})",
            channel(first),
            channel(second),
            channel(third),
            alpha_suffix(alpha)
        );
    }
    write!(
        formatter,
        "{space}({} {} {}{})",
        channel(first),
        channel(second),
        channel(third),
        alpha_suffix(alpha)
    )
}
