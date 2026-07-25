/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Relative color syntax: `rgb(from <color> r g b / alpha)`.
//!
//! CSS Color 5 <https://drafts.csswg.org/css-color-5/#relative-colors>. The
//! origin color is converted into the output function's space, and its
//! channels become numbers bound to single-letter keywords that the channel
//! grammar and any `calc()` inside it may reference.
//!
//! Harvest lift (cutover plan F0), shaped after the fork's origin-color
//! handling in `style/color/parsing.rs` at
//! `b157d925267fdd37b03f43e3387ab2f0909e57b0`.

use super::space::{ColorSpace, Components};
use super::Color;

/// A channel keyword bound to the origin color's value for that channel.
#[derive(Clone, Copy)]
pub struct Binding {
    pub name: &'static str,
    pub value: f32,
}

/// The four bindings a relative color makes available.
pub type Bindings = [Binding; 4];

/// The channel keywords for a space, in component order.
///
/// These are the names CSS Color 5 defines per output function, not a naming
/// convention: `lch()` binds `l c h` while `lab()` binds `l a b`, and the
/// XYZ spaces bind `x y z`.
fn channel_names(space: ColorSpace) -> [&'static str; 3] {
    match space {
        ColorSpace::Hsl => ["h", "s", "l"],
        ColorSpace::Hwb => ["h", "w", "b"],
        ColorSpace::Lab | ColorSpace::Oklab => ["l", "a", "b"],
        ColorSpace::Lch | ColorSpace::Oklch => ["l", "c", "h"],
        ColorSpace::XyzD50 | ColorSpace::XyzD65 => ["x", "y", "z"],
        _ => ["r", "g", "b"],
    }
}

/// How a stored component scales into the number its keyword resolves to.
///
/// Only `rgb()` differs: Livery stores sRGB components in 0-1, but CSS Color
/// 5 says `r`, `g`, and `b` resolve to numbers in 0-255 inside `rgb(from ...)`.
/// `color(from ... srgb ...)` keeps 0-1, which is why this keys on the
/// function rather than on the space alone.
fn keyword_scale(space: ColorSpace, predefined: bool) -> f32 {
    if space == ColorSpace::Srgb && !predefined {
        255.0
    } else {
        1.0
    }
}

/// Bind an origin color's channels for use inside `space`'s channel grammar.
///
/// Returns `None` when the origin cannot be resolved here, which today means
/// `currentcolor` alone: its used value depends on the cascade, so the
/// relative color cannot be computed at parse time. A system color resolves
/// through Livery's bounded palette, so it binds like any absolute color, and
/// will start tracking the device palette when that lane arrives.
pub fn bind(origin: Color, space: ColorSpace, predefined: bool) -> Option<Bindings> {
    let (components, alpha) = origin.components_in(space)?;
    let names = channel_names(space);
    let scale = keyword_scale(space, predefined);
    let Components(first, second, third) = components;
    // A missing component in the origin binds as zero: the keyword has to
    // resolve to a number for the arithmetic to mean anything.
    let resolve = |value: f32| if value.is_nan() { 0.0 } else { value };
    Some([
        Binding {
            name: names[0],
            value: resolve(first) * scale,
        },
        Binding {
            name: names[1],
            value: resolve(second) * scale,
        },
        Binding {
            name: names[2],
            value: resolve(third) * scale,
        },
        Binding {
            name: "alpha",
            value: if alpha.is_nan() { 1.0 } else { alpha },
        },
    ])
}

/// Look up a bound keyword, case-insensitively.
pub fn lookup(bindings: &Bindings, name: &str) -> Option<f32> {
    bindings
        .iter()
        .find(|binding| name.eq_ignore_ascii_case(binding.name))
        .map(|binding| binding.value)
}

/// Substitute bound keywords into a math expression's source text.
///
/// Livery's math program has no notion of channel keywords, so
/// `calc(r * 2)` is rewritten to `calc(128 * 2)` before it reaches the
/// parser. Substitution is whole-identifier only: `r` inside `rem` or
/// `--radius` must not be touched, and neither must a unit.
pub fn substitute(source: &str, bindings: &Bindings) -> String {
    let bytes = source.as_bytes();
    let is_ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if !is_ident(bytes[index]) {
            out.push(bytes[index] as char);
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_ident(bytes[index]) {
            index += 1;
        }
        let word = &source[start..index];
        // A word directly preceded by a digit or dot is a unit, not an
        // identifier: the `s` of `2s` must survive.
        let preceded_by_number = start > 0
            && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.');
        match lookup(bindings, word) {
            Some(value) if !preceded_by_number => {
                out.push_str(&crate::values::format_number(value));
            },
            _ => out.push_str(word),
        }
    }
    out
}
