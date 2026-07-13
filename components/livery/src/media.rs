//! Genet-shaped media environment and first-lane query evaluation.

use std::{error::Error, fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MediaType {
    Screen,
    Print,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ColorScheme {
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReducedMotion {
    NoPreference,
    Reduce,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContrastPreference {
    NoPreference,
    More,
    Less,
    Custom,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReducedTransparency {
    NoPreference,
    Reduce,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InvertedColors {
    None,
    Inverted,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ForcedColors {
    None,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisplayMode {
    Browser,
    Standalone,
    MinimalUi,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Scripting {
    None,
    InitialOnly,
    Enabled,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ColorGamut {
    Srgb,
    P3,
    Rec2020,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UpdateFrequency {
    None,
    Slow,
    Fast,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PointerAccuracy {
    None,
    Coarse,
    Fine,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PointerCapabilities {
    pub accuracy: PointerAccuracy,
    pub hover: bool,
}

impl PointerCapabilities {
    pub const NONE: Self = Self {
        accuracy: PointerAccuracy::None,
        hover: false,
    };
    pub const TOUCH: Self = Self {
        accuracy: PointerAccuracy::Coarse,
        hover: false,
    };
    pub const MOUSE: Self = Self {
        accuracy: PointerAccuracy::Fine,
        hover: true,
    };
}

/// Union of every pointing device, preserving the multi-capability
/// `any-pointer` model needed by hybrid touchscreen/mouse hosts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AnyPointerCapabilities {
    pub coarse: bool,
    pub fine: bool,
    pub hover: bool,
}

impl AnyPointerCapabilities {
    pub const NONE: Self = Self {
        coarse: false,
        fine: false,
        hover: false,
    };
    pub const MOUSE: Self = Self {
        coarse: false,
        fine: true,
        hover: true,
    };
}

/// Host-owned media state. Updating one field never resets the others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Device {
    pub media_type: MediaType,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub color_scheme: ColorScheme,
    pub reduced_motion: ReducedMotion,
    pub contrast: ContrastPreference,
    pub reduced_transparency: ReducedTransparency,
    pub inverted_colors: InvertedColors,
    pub forced_colors: ForcedColors,
    pub display_mode: DisplayMode,
    pub scripting: Scripting,
    pub color_gamut: ColorGamut,
    pub update: UpdateFrequency,
    pub primary_pointer: PointerCapabilities,
    pub any_pointer: AnyPointerCapabilities,
}

impl Device {
    pub fn screen(viewport_width: f32, viewport_height: f32) -> Self {
        Self {
            media_type: MediaType::Screen,
            viewport_width,
            viewport_height,
            color_scheme: ColorScheme::Light,
            reduced_motion: ReducedMotion::NoPreference,
            contrast: ContrastPreference::NoPreference,
            reduced_transparency: ReducedTransparency::NoPreference,
            inverted_colors: InvertedColors::None,
            forced_colors: ForcedColors::None,
            display_mode: DisplayMode::Browser,
            scripting: Scripting::Enabled,
            color_gamut: ColorGamut::Srgb,
            update: UpdateFrequency::Fast,
            primary_pointer: PointerCapabilities::MOUSE,
            any_pointer: AnyPointerCapabilities::MOUSE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dimension {
    Width,
    Height,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Range {
    Equal,
    Minimum,
    Maximum,
}

#[derive(Clone, Debug, PartialEq)]
enum MediaFeature {
    Unknown,
    Dimension {
        dimension: Dimension,
        range: Range,
        css_px: f32,
    },
    OrientationPortrait,
    OrientationLandscape,
    ColorScheme(ColorScheme),
    ReducedMotion(ReducedMotion),
    Contrast(ContrastPreference),
    ReducedTransparency(ReducedTransparency),
    InvertedColors(InvertedColors),
    ForcedColors(ForcedColors),
    DisplayMode(DisplayMode),
    Scripting(Scripting),
    ColorGamut(ColorGamut),
    Update(UpdateFrequency),
    Pointer(PointerAccuracy),
    Hover(bool),
    AnyPointer(PointerAccuracy),
    AnyHover(bool),
}

impl MediaFeature {
    fn matches(&self, device: &Device) -> bool {
        match *self {
            Self::Unknown => false,
            Self::Dimension {
                dimension,
                range,
                css_px,
            } => {
                let actual = match dimension {
                    Dimension::Width => device.viewport_width,
                    Dimension::Height => device.viewport_height,
                };
                match range {
                    Range::Equal => actual == css_px,
                    Range::Minimum => actual >= css_px,
                    Range::Maximum => actual <= css_px,
                }
            },
            Self::OrientationPortrait => device.viewport_height >= device.viewport_width,
            Self::OrientationLandscape => device.viewport_width > device.viewport_height,
            Self::ColorScheme(value) => device.color_scheme == value,
            Self::ReducedMotion(value) => device.reduced_motion == value,
            Self::Contrast(value) => device.contrast == value,
            Self::ReducedTransparency(value) => device.reduced_transparency == value,
            Self::InvertedColors(value) => device.inverted_colors == value,
            Self::ForcedColors(value) => device.forced_colors == value,
            Self::DisplayMode(value) => device.display_mode == value,
            Self::Scripting(value) => device.scripting == value,
            Self::ColorGamut(value) => device.color_gamut >= value,
            Self::Update(value) => device.update == value,
            Self::Pointer(value) => device.primary_pointer.accuracy == value,
            Self::Hover(value) => device.primary_pointer.hover == value,
            Self::AnyPointer(PointerAccuracy::None) => {
                !device.any_pointer.coarse && !device.any_pointer.fine
            },
            Self::AnyPointer(PointerAccuracy::Coarse) => device.any_pointer.coarse,
            Self::AnyPointer(PointerAccuracy::Fine) => device.any_pointer.fine,
            Self::AnyHover(value) => device.any_pointer.hover == value,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct MediaQuery {
    negated: bool,
    media_type: Option<MediaType>,
    features: Vec<MediaFeature>,
}

impl MediaQuery {
    fn matches(&self, device: &Device) -> bool {
        let type_matches = self.media_type.is_none_or(|kind| kind == device.media_type);
        let matches = type_matches && self.features.iter().all(|feature| feature.matches(device));
        matches != self.negated
    }
}

/// A comma-separated media query list. Queries are ORed; features joined with
/// `and` inside one query are ANDed.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaQueryList(Vec<MediaQuery>);

impl MediaQueryList {
    pub fn matches(&self, device: &Device) -> bool {
        self.0.iter().any(|query| query.matches(device))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaParseError {
    detail: String,
}

impl MediaParseError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for MediaParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for MediaParseError {}

fn split_queries(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0_u32;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&input[start..index]);
                start = index + 1;
            },
            _ => {},
        }
    }
    result.push(&input[start..]);
    result
}

fn parse_keyword<T: Copy>(
    value: &str,
    expected: &'static str,
    keywords: &[(&str, T)],
) -> Result<T, MediaParseError> {
    keywords
        .iter()
        .find(|(keyword, _)| value.eq_ignore_ascii_case(keyword))
        .map(|(_, value)| *value)
        .ok_or_else(|| MediaParseError::new(format!("expected {expected}")))
}

fn parse_css_px(value: &str) -> Result<f32, MediaParseError> {
    let number = value
        .trim()
        .strip_suffix("px")
        .ok_or_else(|| MediaParseError::new("media dimensions currently require px"))?
        .trim()
        .parse::<f32>()
        .map_err(|_| MediaParseError::new("invalid media dimension"))?;
    if number.is_finite() && number >= 0.0 {
        Ok(number)
    } else {
        Err(MediaParseError::new(
            "media dimensions must be finite and non-negative",
        ))
    }
}

fn parse_feature(input: &str) -> Result<MediaFeature, MediaParseError> {
    let (name, value) = input
        .split_once(':')
        .ok_or_else(|| MediaParseError::new("media features require a value"))?;
    let name = name.trim().to_ascii_lowercase();
    let value = value.trim();
    let dimension = |dimension, range| {
        parse_css_px(value).map(|css_px| MediaFeature::Dimension {
            dimension,
            range,
            css_px,
        })
    };
    match name.as_str() {
        "width" => dimension(Dimension::Width, Range::Equal),
        "min-width" => dimension(Dimension::Width, Range::Minimum),
        "max-width" => dimension(Dimension::Width, Range::Maximum),
        "height" => dimension(Dimension::Height, Range::Equal),
        "min-height" => dimension(Dimension::Height, Range::Minimum),
        "max-height" => dimension(Dimension::Height, Range::Maximum),
        "orientation" => match value.to_ascii_lowercase().as_str() {
            "portrait" => Ok(MediaFeature::OrientationPortrait),
            "landscape" => Ok(MediaFeature::OrientationLandscape),
            _ => Err(MediaParseError::new("expected portrait or landscape")),
        },
        "prefers-color-scheme" => parse_keyword(
            value,
            "light or dark",
            &[("light", ColorScheme::Light), ("dark", ColorScheme::Dark)],
        )
        .map(MediaFeature::ColorScheme),
        "prefers-reduced-motion" => parse_keyword(
            value,
            "no-preference or reduce",
            &[
                ("no-preference", ReducedMotion::NoPreference),
                ("reduce", ReducedMotion::Reduce),
            ],
        )
        .map(MediaFeature::ReducedMotion),
        "prefers-contrast" => parse_keyword(
            value,
            "a contrast preference",
            &[
                ("no-preference", ContrastPreference::NoPreference),
                ("more", ContrastPreference::More),
                ("less", ContrastPreference::Less),
                ("custom", ContrastPreference::Custom),
            ],
        )
        .map(MediaFeature::Contrast),
        "prefers-reduced-transparency" => parse_keyword(
            value,
            "no-preference or reduce",
            &[
                ("no-preference", ReducedTransparency::NoPreference),
                ("reduce", ReducedTransparency::Reduce),
            ],
        )
        .map(MediaFeature::ReducedTransparency),
        "inverted-colors" => parse_keyword(
            value,
            "none or inverted",
            &[
                ("none", InvertedColors::None),
                ("inverted", InvertedColors::Inverted),
            ],
        )
        .map(MediaFeature::InvertedColors),
        "forced-colors" => parse_keyword(
            value,
            "none or active",
            &[
                ("none", ForcedColors::None),
                ("active", ForcedColors::Active),
            ],
        )
        .map(MediaFeature::ForcedColors),
        "display-mode" => parse_keyword(
            value,
            "a display mode",
            &[
                ("browser", DisplayMode::Browser),
                ("standalone", DisplayMode::Standalone),
                ("minimal-ui", DisplayMode::MinimalUi),
                ("fullscreen", DisplayMode::Fullscreen),
            ],
        )
        .map(MediaFeature::DisplayMode),
        "scripting" => parse_keyword(
            value,
            "none, initial-only, or enabled",
            &[
                ("none", Scripting::None),
                ("initial-only", Scripting::InitialOnly),
                ("enabled", Scripting::Enabled),
            ],
        )
        .map(MediaFeature::Scripting),
        "color-gamut" => parse_keyword(
            value,
            "srgb, p3, or rec2020",
            &[
                ("srgb", ColorGamut::Srgb),
                ("p3", ColorGamut::P3),
                ("rec2020", ColorGamut::Rec2020),
            ],
        )
        .map(MediaFeature::ColorGamut),
        "update" => parse_keyword(
            value,
            "none, slow, or fast",
            &[
                ("none", UpdateFrequency::None),
                ("slow", UpdateFrequency::Slow),
                ("fast", UpdateFrequency::Fast),
            ],
        )
        .map(MediaFeature::Update),
        "pointer" => parse_keyword(
            value,
            "none, coarse, or fine",
            &[
                ("none", PointerAccuracy::None),
                ("coarse", PointerAccuracy::Coarse),
                ("fine", PointerAccuracy::Fine),
            ],
        )
        .map(MediaFeature::Pointer),
        "hover" => parse_keyword(value, "none or hover", &[("none", false), ("hover", true)])
            .map(MediaFeature::Hover),
        "any-pointer" => parse_keyword(
            value,
            "none, coarse, or fine",
            &[
                ("none", PointerAccuracy::None),
                ("coarse", PointerAccuracy::Coarse),
                ("fine", PointerAccuracy::Fine),
            ],
        )
        .map(MediaFeature::AnyPointer),
        "any-hover" => parse_keyword(value, "none or hover", &[("none", false), ("hover", true)])
            .map(MediaFeature::AnyHover),
        _ => Ok(MediaFeature::Unknown),
    }
}

fn strip_trailing_and(input: &str) -> &str {
    let input = input.trim_end();
    let split = input.len().saturating_sub(3);
    let Some(suffix) = input.get(split..) else {
        return input;
    };
    let Some(prefix) = input.get(..split) else {
        return input;
    };

    if suffix.eq_ignore_ascii_case("and") && prefix.ends_with(char::is_whitespace) {
        prefix.trim_end()
    } else {
        input
    }
}

fn strip_leading_and(input: &str) -> Option<&str> {
    let input = input.trim_start();
    let prefix = input.get(..3)?;
    let rest = input.get(3..)?;
    (prefix.eq_ignore_ascii_case("and") && rest.starts_with(char::is_whitespace))
        .then(|| rest.trim_start())
}

fn parse_query(input: &str) -> Result<MediaQuery, MediaParseError> {
    let mut input = input.trim();
    let negated = input
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("not"))
        && input[3..].starts_with(char::is_whitespace);
    if negated {
        input = input[3..].trim_start();
    }

    let first_feature = input.find('(').unwrap_or(input.len());
    let prefix = strip_trailing_and(input[..first_feature].trim());
    let media_type = if prefix.is_empty() || prefix.eq_ignore_ascii_case("all") {
        None
    } else if prefix.eq_ignore_ascii_case("screen") {
        Some(MediaType::Screen)
    } else if prefix.eq_ignore_ascii_case("print") {
        Some(MediaType::Print)
    } else {
        return Err(MediaParseError::new(format!("unknown media type {prefix}")));
    };

    let mut features = Vec::new();
    let mut rest = &input[first_feature..];
    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        if let Some(after_and) = strip_leading_and(rest) {
            rest = after_and;
        }
        if !rest.starts_with('(') {
            return Err(MediaParseError::new(
                "expected a parenthesized media feature",
            ));
        }
        let close = rest
            .find(')')
            .ok_or_else(|| MediaParseError::new("unclosed media feature"))?;
        features.push(parse_feature(&rest[1..close])?);
        rest = &rest[close + 1..];
    }
    if media_type.is_none() && features.is_empty() {
        return Err(MediaParseError::new("empty media query"));
    }
    Ok(MediaQuery {
        negated,
        media_type,
        features,
    })
}

impl FromStr for MediaQueryList {
    type Err = MediaParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        split_queries(input)
            .into_iter()
            .map(parse_query)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}
