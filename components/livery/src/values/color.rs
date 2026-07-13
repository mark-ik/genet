use std::{fmt, str::FromStr};

use cssparser::color::{parse_hash_color, parse_named_color};

use super::{ParseError, format_number};

/// Colors needed by Livery's first lane.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Color {
    Transparent,
    CurrentColor,
    CanvasText,
    Rgba {
        red: u8,
        green: u8,
        blue: u8,
        alpha: u8,
    },
}

impl Color {
    fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self::Rgba {
            red,
            green,
            blue,
            alpha,
        }
    }

    fn parse_component(input: &str) -> Result<u8, ParseError> {
        let value = if let Some(percentage) = input.strip_suffix('%') {
            percentage
                .parse::<f32>()
                .map(|value| value * 2.55)
                .map_err(|_| ParseError::expected("an RGB component"))?
        } else {
            input
                .parse::<f32>()
                .map_err(|_| ParseError::expected("an RGB component"))?
        };
        if !value.is_finite() || !(0.0..=255.0).contains(&value) {
            return Err(ParseError::expected("an RGB component from 0 through 255"));
        }
        Ok(value.round() as u8)
    }

    fn parse_alpha(input: &str) -> Result<u8, ParseError> {
        let value = if let Some(percentage) = input.strip_suffix('%') {
            percentage
                .parse::<f32>()
                .map(|value| value / 100.0)
                .map_err(|_| ParseError::expected("an alpha component"))?
        } else {
            input
                .parse::<f32>()
                .map_err(|_| ParseError::expected("an alpha component"))?
        };
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ParseError::expected("an alpha component from 0 through 1"));
        }
        Ok((value * 255.0).round() as u8)
    }

    fn parse_function(input: &str) -> Result<Self, ParseError> {
        let open = input
            .find('(')
            .ok_or_else(|| ParseError::expected("rgb() or rgba()"))?;
        if !input.ends_with(')') {
            return Err(ParseError::expected("a closed rgb() function"));
        }
        let name = &input[..open];
        if !name.eq_ignore_ascii_case("rgb") && !name.eq_ignore_ascii_case("rgba") {
            return Err(ParseError::expected("rgb() or rgba()"));
        }
        let body = input[open + 1..input.len() - 1]
            .replace(',', " ")
            .replace('/', " / ");
        let parts = body.split_ascii_whitespace().collect::<Vec<_>>();
        let slash = parts.iter().position(|part| *part == "/");
        let (channels, alpha) = if let Some(slash) = slash {
            (&parts[..slash], parts.get(slash + 1).copied())
        } else if name.eq_ignore_ascii_case("rgba") && parts.len() == 4 {
            (&parts[..3], parts.get(3).copied())
        } else {
            (&parts[..], None)
        };
        if channels.len() != 3 || slash.is_some_and(|index| parts.len() != index + 2) {
            return Err(ParseError::expected(
                "three RGB channels and optional alpha",
            ));
        }
        Ok(Self::rgba(
            Self::parse_component(channels[0])?,
            Self::parse_component(channels[1])?,
            Self::parse_component(channels[2])?,
            alpha.map(Self::parse_alpha).transpose()?.unwrap_or(255),
        ))
    }
}

impl FromStr for Color {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("transparent") {
            return Ok(Self::Transparent);
        }
        if input.eq_ignore_ascii_case("currentcolor") {
            return Ok(Self::CurrentColor);
        }
        if input.eq_ignore_ascii_case("canvastext") {
            return Ok(Self::CanvasText);
        }
        if let Some(hash) = input.strip_prefix('#') {
            let (red, green, blue, alpha) = parse_hash_color(hash.as_bytes())
                .map_err(|_| ParseError::expected("a 3, 4, 6, or 8 digit hex color"))?;
            return Ok(Self::rgba(red, green, blue, (alpha * 255.0).round() as u8));
        }
        if input.contains('(') {
            return Self::parse_function(input);
        }
        if let Ok((red, green, blue)) = parse_named_color(input) {
            return Ok(Self::rgba(red, green, blue, 255));
        }
        Err(ParseError::expected("a color"))
    }
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Transparent => formatter.write_str("transparent"),
            Self::CurrentColor => formatter.write_str("currentcolor"),
            Self::CanvasText => formatter.write_str("CanvasText"),
            Self::Rgba {
                red,
                green,
                blue,
                alpha: 255,
            } => write!(formatter, "#{red:02x}{green:02x}{blue:02x}"),
            Self::Rgba {
                red,
                green,
                blue,
                alpha,
            } => write!(
                formatter,
                "rgba({red}, {green}, {blue}, {})",
                format_number(f32::from(alpha) / 255.0)
            ),
        }
    }
}
