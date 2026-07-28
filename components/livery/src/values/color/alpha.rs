/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Color 5's `alpha()` relative color function.
//!
//! <https://drafts.csswg.org/css-color-5/#relative-alpha>

use cssparser::{ParseError as CssParseError, Parser};

use super::relative;
use super::{Color, ColorSpace};

type Failure<'i> = CssParseError<'i, ()>;

/// Parse `alpha(from <color> [ / <alpha-value> | none ]?)`.
pub fn parse_alpha<'i>(input: &mut Parser<'i, '_>) -> Result<Color, Failure<'i>> {
    input.expect_ident_matching("from")?;
    let origin = super::parse::parse_from(input)?;
    let bindings =
        relative::bind(origin, ColorSpace::Srgb, true).ok_or_else(|| input.new_custom_error(()))?;
    let alpha = if input.try_parse(|nested| nested.expect_delim('/')).is_ok() {
        super::parse::parse_alpha(input, Some(&bindings))?.unwrap_or(f32::NAN)
    } else {
        relative::lookup(&bindings, "alpha").unwrap_or(1.0)
    };
    origin
        .with_alpha(alpha)
        .ok_or_else(|| input.new_custom_error(()))
}
