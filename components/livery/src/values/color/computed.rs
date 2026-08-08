/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The computed-value form of a CSS color.
//!
//! [`SpecifiedColor`] retains authored syntax. This type is the sole form
//! stored by color-bearing computed properties: it preserves a contextual
//! expression until the consumer supplies the foreground and system-color
//! context needed to turn it into a numeric [`Color`].

use std::{fmt, str::FromStr};

use super::{Color, SpecifiedColor, SystemColor};
use crate::values::ParseError;

/// A color after the specified-to-computed boundary.
///
/// `Expression` deliberately owns the retained syntax instead of keeping a
/// numeric `Color` beside it. That makes one value authoritative through
/// declarations, substitution, inheritance, and generated field copies.
#[derive(Clone, Debug, PartialEq)]
pub enum ComputedColor {
    Absolute(Color),
    CurrentColor,
    System(SystemColor),
    Expression(Box<SpecifiedColor>),
}

/// The information a color consumer supplies at used-value time.
///
/// C1 keeps the legacy palette only behind this explicit context so parsing
/// and computed values perform no palette lookup. C2 replaces the legacy
/// resolver with element scheme and host palette selection.
#[derive(Clone, Copy)]
pub struct UsedColorContext {
    current_color: Color,
    system_color: fn(SystemColor) -> Color,
}

impl UsedColorContext {
    pub fn new(current_color: Color, system_color: fn(SystemColor) -> Color) -> Self {
        Self {
            current_color,
            system_color,
        }
    }

    /// Compatibility context for consumers not yet migrated to C2's host
    /// palette. It is a lowering policy, never a parser or computed-value
    /// shortcut.
    pub fn legacy(current_color: Color) -> Self {
        Self::new(current_color, legacy_system_color)
    }
}

impl ComputedColor {
    pub const TRANSPARENT: Self = Self::Absolute(Color::TRANSPARENT);
    pub const CURRENT_COLOR: Self = Self::CurrentColor;
    pub const CANVAS_TEXT: Self = Self::System(SystemColor::CanvasText);

    pub fn resolve_used(&self, context: UsedColorContext) -> Color {
        match self {
            Self::Absolute(color) => *color,
            Self::CurrentColor => context.current_color,
            Self::System(system) => (context.system_color)(*system),
            Self::Expression(expression) => resolve_expression(expression, context),
        }
    }

    /// Numeric access for a color that was already absolute at computed-value
    /// time. Contextual values intentionally return `None`; callers that have
    /// a foreground and palette must use [`Self::resolve_used`].
    pub fn to_srgb(&self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Self::Absolute(color) => color.to_srgb(),
            Self::CurrentColor | Self::System(_) | Self::Expression(_) => None,
        }
    }

    pub fn to_srgb8(&self) -> Option<(u8, u8, u8, u8)> {
        match self {
            Self::Absolute(color) => color.to_srgb8(),
            Self::CurrentColor | Self::System(_) | Self::Expression(_) => None,
        }
    }

    /// CSS Color interpolation requires used endpoints. C3 supplies that
    /// context to animation. Until then, only already absolute endpoints can
    /// interpolate; contextual values retain their expression and switch at
    /// the midpoint rather than being flattened or painted as black.
    pub fn interpolate(&self, other: &Self, progress: f32) -> Self {
        match (self, other) {
            (Self::Absolute(from), Self::Absolute(to)) => {
                Self::Absolute(from.interpolate(*to, progress))
            },
            _ if progress.clamp(0.0, 1.0) < 0.5 => self.clone(),
            _ => other.clone(),
        }
    }
}

impl From<Color> for ComputedColor {
    fn from(color: Color) -> Self {
        match color {
            Color::CurrentColor => Self::CurrentColor,
            Color::System(system) => Self::System(system),
            Color::Absolute { .. } => Self::Absolute(color),
        }
    }
}

impl FromStr for ComputedColor {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let specified = input.parse::<SpecifiedColor>()?;
        Ok(match specified.resolved() {
            Some(color) => Self::from(color),
            None => Self::Expression(Box::new(specified)),
        })
    }
}

impl fmt::Display for ComputedColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(color) => color.fmt(formatter),
            Self::CurrentColor => formatter.write_str("currentcolor"),
            Self::System(system) => formatter.write_str(system.css_name()),
            Self::Expression(expression) => expression.fmt(formatter),
        }
    }
}

impl PartialEq<Color> for ComputedColor {
    fn eq(&self, other: &Color) -> bool {
        matches!(self, Self::Absolute(color) if color == other)
            || matches!(self, Self::CurrentColor if *other == Color::CurrentColor)
            || matches!(self, Self::System(system) if *other == Color::System(*system))
    }
}

impl PartialEq<ComputedColor> for Color {
    fn eq(&self, other: &ComputedColor) -> bool {
        other == self
    }
}

fn resolve_expression(expression: &SpecifiedColor, context: UsedColorContext) -> Color {
    let mut css = expression.to_string();
    replace_keyword(&mut css, "currentcolor", &context.current_color.to_string());
    for system in SystemColor::all() {
        replace_keyword(
            &mut css,
            system.css_name(),
            &(context.system_color)(system).to_string(),
        );
    }
    css.parse::<Color>()
        .expect("a validated specified color resolves after its used context is supplied")
}

fn replace_keyword(source: &mut String, keyword: &str, replacement: &str) {
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        let matches = bytes[index..]
            .get(..keyword.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword.as_bytes()));
        let before_is_ident = index > 0 && is_ident(bytes[index - 1]);
        let after = index + keyword.len();
        let after_is_ident = after < bytes.len() && is_ident(bytes[after]);
        if matches && !before_is_ident && !after_is_ident {
            output.push_str(replacement);
            index = after;
        } else {
            let ch = source[index..]
                .chars()
                .next()
                .expect("index remains on a character boundary");
            output.push(ch);
            index += ch.len_utf8();
        }
    }
    *source = output;
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn legacy_system_color(system: SystemColor) -> Color {
    let (red, green, blue) = match system {
        SystemColor::Canvas | SystemColor::ButtonFace | SystemColor::Field => (255, 255, 255),
        SystemColor::LinkText => (0, 0, 238),
        SystemColor::VisitedText => (85, 26, 139),
        SystemColor::ActiveText => (255, 0, 0),
        SystemColor::GrayText => (102, 102, 102),
        SystemColor::Highlight | SystemColor::SelectedItem | SystemColor::AccentColor => {
            (33, 96, 205)
        },
        SystemColor::HighlightText
        | SystemColor::SelectedItemText
        | SystemColor::AccentColorText => (255, 255, 255),
        SystemColor::Mark => (255, 255, 0),
        _ => (0, 0, 0),
    };
    Color::srgb8(red, green, blue, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextual_expression_stays_retained_until_used_resolution() {
        let color = "color-mix(in srgb, currentcolor 100%, white)"
            .parse::<ComputedColor>()
            .unwrap();
        assert!(matches!(color, ComputedColor::Expression(_)));
        assert_eq!(
            color
                .resolve_used(UsedColorContext::legacy("#3568b8".parse().unwrap()))
                .to_srgb(),
            "#3568b8".parse::<Color>().unwrap().to_srgb()
        );
    }

    #[test]
    fn replacement_does_not_touch_an_identifier_substring() {
        let mut value = "color(currentcolorx currentcolor)".to_owned();
        replace_keyword(&mut value, "currentcolor", "red");
        assert_eq!(value, "color(currentcolorx red)");
    }
}
