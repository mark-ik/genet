use std::{fmt, str::FromStr};

use super::{Color, Length, LengthPercentage, Matrix2D, ParseError, format_number, keyword_value};

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundImage {
    None,
    LinearGradient { from: Color, to: Color },
    Url(Box<str>),
}

impl BackgroundImage {
    /// Interpolate the bounded single-image forms consumed by the retained
    /// paint lane. Two-stop linear gradients interpolate each stop; URLs and
    /// mixed image shapes remain discrete until the image-list ratchet.
    pub fn interpolate(&self, other: &Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        let value = match (self, other) {
            (
                Self::LinearGradient { from, to },
                Self::LinearGradient {
                    from: other_from,
                    to: other_to,
                },
            ) => Some(Self::LinearGradient {
                from: from.interpolate(*other_from, progress),
                to: to.interpolate(*other_to, progress),
            }),
            _ => None,
        };
        value.unwrap_or_else(|| {
            if progress < 0.5 {
                self.clone()
            } else {
                other.clone()
            }
        })
    }
}

/// The bounded background-position pair consumed by the paint lane. Lengths,
/// percentages, and the five physical position keywords are accepted; the
/// full four-value grammar remains outside this ratchet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundPosition {
    pub x: LengthPercentage,
    pub y: LengthPercentage,
}

impl BackgroundPosition {
    pub const ZERO: Self = Self {
        x: LengthPercentage::ZERO,
        y: LengthPercentage::ZERO,
    };

    /// Interpolate the bounded two-component position used by the retained
    /// background image lane. Each component keeps the shared
    /// length/percentage unit boundary.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        Self {
            x: self.x.interpolate(other.x, progress),
            y: self.y.interpolate(other.y, progress),
        }
    }
}

fn position_component(input: &str, horizontal: bool) -> Result<LengthPercentage, ParseError> {
    let value = input.trim();
    let keyword = if value.eq_ignore_ascii_case("center") {
        Some(LengthPercentage::Percentage(0.5))
    } else if horizontal && value.eq_ignore_ascii_case("left") {
        Some(LengthPercentage::ZERO)
    } else if horizontal && value.eq_ignore_ascii_case("right") {
        Some(LengthPercentage::Percentage(1.0))
    } else if !horizontal && value.eq_ignore_ascii_case("top") {
        Some(LengthPercentage::ZERO)
    } else if !horizontal && value.eq_ignore_ascii_case("bottom") {
        Some(LengthPercentage::Percentage(1.0))
    } else {
        None
    };
    keyword.map_or_else(|| value.parse(), Ok)
}

impl FromStr for BackgroundPosition {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let values = input.split_ascii_whitespace().collect::<Vec<_>>();
        match values.as_slice() {
            [value] => {
                if value.eq_ignore_ascii_case("top") || value.eq_ignore_ascii_case("bottom") {
                    Ok(Self {
                        x: LengthPercentage::Percentage(0.5),
                        y: position_component(value, false)?,
                    })
                } else {
                    Ok(Self {
                        x: position_component(value, true)?,
                        y: LengthPercentage::Percentage(0.5),
                    })
                }
            },
            [first, second]
                if (first.eq_ignore_ascii_case("top") || first.eq_ignore_ascii_case("bottom"))
                    && (second.eq_ignore_ascii_case("left")
                        || second.eq_ignore_ascii_case("right")) =>
            {
                Ok(Self {
                    x: position_component(second, true)?,
                    y: position_component(first, false)?,
                })
            },
            [first, second] => Ok(Self {
                x: position_component(first, true)?,
                y: position_component(second, false)?,
            }),
            _ => Err(ParseError::expected(
                "one or two background-position values",
            )),
        }
    }
}

impl fmt::Display for BackgroundPosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.x, self.y)
    }
}

keyword_value! {
    /// Bounded background tiling modes. `space` and `round` remain future
    /// additions because they require intrinsic-size adjustment rules.
pub enum BackgroundRepeat {
        Repeat => "repeat",
        NoRepeat => "no-repeat",
        RepeatX => "repeat-x",
        RepeatY => "repeat-y",
    }
}

impl BackgroundRepeat {
    /// Repeat modes are discrete in CSS. The retained transition switches to
    /// the target mode at the midpoint of the clock.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        if progress.clamp(0.0, 1.0) < 0.5 {
            self
        } else {
            other
        }
    }
}

impl FromStr for BackgroundImage {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        if input.len() > 5 && input[..4].eq_ignore_ascii_case("url(") && input.ends_with(')') {
            let raw = input[4..input.len() - 1].trim();
            let url = raw
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    raw.strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(raw)
                .trim();
            if !url.is_empty() {
                return Ok(Self::Url(url.into()));
            }
        }
        let Some(arguments) = input
            .strip_prefix("linear-gradient(")
            .and_then(|value| value.strip_suffix(')'))
        else {
            return Err(ParseError::expected(
                "none, url(<image>), or a two-stop linear-gradient",
            ));
        };
        let mut colors = arguments.split(',').map(str::trim);
        let from = colors
            .next()
            .ok_or_else(|| ParseError::expected("two gradient colors"))?
            .parse::<Color>()?;
        let to = colors
            .next()
            .ok_or_else(|| ParseError::expected("two gradient colors"))?
            .parse::<Color>()?;
        if colors.next().is_some() {
            return Err(ParseError::expected("two gradient colors"));
        }
        Ok(Self::LinearGradient { from, to })
    }
}

impl fmt::Display for BackgroundImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::LinearGradient { from, to } => {
                write!(formatter, "linear-gradient({from}, {to})")
            },
            Self::Url(url) => write!(formatter, "url({url})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Duration(f32);

impl Duration {
    pub const ZERO: Self = Self(0.0);

    pub const fn milliseconds(self) -> f32 {
        self.0
    }
}

impl FromStr for Duration {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim().to_ascii_lowercase();
        let (number, multiplier) = if let Some(value) = input.strip_suffix("ms") {
            (value, 1.0)
        } else if let Some(value) = input.strip_suffix('s') {
            (value, 1_000.0)
        } else if input == "0" {
            ("0", 1.0)
        } else {
            return Err(ParseError::expected("a non-negative CSS duration"));
        };
        let value = number
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ParseError::expected("a non-negative CSS duration"))?;
        Ok(Self(value * multiplier))
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}ms", format_number(self.0))
    }
}

/// A bounded CSS animation name. The first animation gate accepts one custom
/// identifier or `none`; comma-separated animation lists remain outside the
/// lane.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationName {
    None,
    Name(Box<str>),
}

impl AnimationName {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Name(name) => Some(name),
        }
    }
}

impl FromStr for AnimationName {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let valid = !input.is_empty()
            && input.chars().enumerate().all(|(index, ch)| {
                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') && index > 0
            })
            && input
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, '_' | '-'));
        if valid {
            Ok(Self::Name(input.into()))
        } else {
            Err(ParseError::expected("none or a custom animation name"))
        }
    }
}

impl fmt::Display for AnimationName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Name(name) => formatter.write_str(name),
        }
    }
}

/// The supported transition-property set consumed by the retained paint clock.
/// Explicit lists retain their property bitset so new combinations do not
/// silently widen to `all`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransitionProperty {
    All,
    None,
    Opacity,
    BackgroundColor,
    Color,
    BorderTopColor,
    BorderBottomColor,
    BorderLeftColor,
    BorderRightColor,
    BorderTopWidth,
    BorderBottomWidth,
    BorderLeftWidth,
    BorderRightWidth,
    BorderRadius,
    Transform,
    BackgroundPosition,
    BoxShadow,
    BackgroundImage,
    BorderTopStyle,
    BorderBottomStyle,
    BorderLeftStyle,
    BorderRightStyle,
    BackgroundRepeat,
    List(u32),
    OpacityAndBackgroundColor,
    OpacityAndColor,
    BackgroundColorAndColor,
    OpacityAndBackgroundColorAndColor,
}

impl FromStr for TransitionProperty {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let mut flags = 0_u32;
        let mut saw_item = false;
        for item in input.split(',') {
            saw_item = true;
            let bit = match item.trim().to_ascii_lowercase().as_str() {
                "opacity" => 1,
                "background-color" => 2,
                "color" => 4,
                "border-top-color" => 8,
                "border-bottom-color" => 16,
                "border-left-color" => 32,
                "border-right-color" => 64,
                "border-top-width" => 2048,
                "border-bottom-width" => 4096,
                "border-left-width" => 8192,
                "border-right-width" => 16384,
                "border-radius" => 128,
                "transform" => 256,
                "background-position" => 512,
                "box-shadow" => 1024,
                "background-image" => 32768,
                "border-top-style" => 65536,
                "border-bottom-style" => 131072,
                "border-left-style" => 262144,
                "border-right-style" => 524288,
                "background-repeat" => 1048576,
                _ => return Err(ParseError::expected("a bounded transition-property list")),
            };
            if flags & bit != 0 {
                return Err(ParseError::expected("a bounded transition-property list"));
            }
            flags |= bit;
        }
        if !saw_item {
            return Err(ParseError::expected("a bounded transition-property list"));
        }
        Self::from_flags(flags)
            .ok_or_else(|| ParseError::expected("a supported transition-property list"))
    }
}

impl fmt::Display for TransitionProperty {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = match self {
            Self::All => return formatter.write_str("all"),
            Self::None => return formatter.write_str("none"),
            Self::Opacity => "opacity",
            Self::BackgroundColor => "background-color",
            Self::Color => "color",
            Self::BorderTopColor => "border-top-color",
            Self::BorderBottomColor => "border-bottom-color",
            Self::BorderLeftColor => "border-left-color",
            Self::BorderRightColor => "border-right-color",
            Self::BorderTopWidth => "border-top-width",
            Self::BorderBottomWidth => "border-bottom-width",
            Self::BorderLeftWidth => "border-left-width",
            Self::BorderRightWidth => "border-right-width",
            Self::BorderRadius => "border-radius",
            Self::Transform => "transform",
            Self::BackgroundPosition => "background-position",
            Self::BoxShadow => "box-shadow",
            Self::BackgroundImage => "background-image",
            Self::BorderTopStyle => "border-top-style",
            Self::BorderBottomStyle => "border-bottom-style",
            Self::BorderLeftStyle => "border-left-style",
            Self::BorderRightStyle => "border-right-style",
            Self::BackgroundRepeat => "background-repeat",
            Self::OpacityAndBackgroundColor => "opacity, background-color",
            Self::OpacityAndColor => "opacity, color",
            Self::BackgroundColorAndColor => "background-color, color",
            Self::OpacityAndBackgroundColorAndColor => "opacity, background-color, color",
            Self::List(flags) => {
                let mut first = true;
                for (bit, name) in [
                    (1, "opacity"),
                    (2, "background-color"),
                    (4, "color"),
                    (8, "border-top-color"),
                    (16, "border-bottom-color"),
                    (32, "border-left-color"),
                    (64, "border-right-color"),
                    (2048, "border-top-width"),
                    (4096, "border-bottom-width"),
                    (8192, "border-left-width"),
                    (16384, "border-right-width"),
                    (128, "border-radius"),
                    (256, "transform"),
                    (512, "background-position"),
                    (1024, "box-shadow"),
                    (32768, "background-image"),
                    (65536, "border-top-style"),
                    (131072, "border-bottom-style"),
                    (262144, "border-left-style"),
                    (524288, "border-right-style"),
                    (1048576, "background-repeat"),
                ] {
                    if flags & bit == 0 {
                        continue;
                    }
                    if !first {
                        formatter.write_str(", ")?;
                    }
                    formatter.write_str(name)?;
                    first = false;
                }
                return Ok(());
            },
        };
        formatter.write_str(names)
    }
}

impl TransitionProperty {
    fn from_flags(flags: u32) -> Option<Self> {
        Some(match flags {
            1 => Self::Opacity,
            2 => Self::BackgroundColor,
            4 => Self::Color,
            8 => Self::BorderTopColor,
            16 => Self::BorderBottomColor,
            32 => Self::BorderLeftColor,
            64 => Self::BorderRightColor,
            2048 => Self::BorderTopWidth,
            4096 => Self::BorderBottomWidth,
            8192 => Self::BorderLeftWidth,
            16384 => Self::BorderRightWidth,
            128 => Self::BorderRadius,
            256 => Self::Transform,
            512 => Self::BackgroundPosition,
            1024 => Self::BoxShadow,
            32768 => Self::BackgroundImage,
            65536 => Self::BorderTopStyle,
            131072 => Self::BorderBottomStyle,
            262144 => Self::BorderLeftStyle,
            524288 => Self::BorderRightStyle,
            1048576 => Self::BackgroundRepeat,
            3 => Self::OpacityAndBackgroundColor,
            5 => Self::OpacityAndColor,
            6 => Self::BackgroundColorAndColor,
            7 => Self::OpacityAndBackgroundColorAndColor,
            _ if flags != 0 => Self::List(flags),
            _ => return None,
        })
    }

    fn includes_flag(self, bit: u32) -> bool {
        matches!(self, Self::All) || self.flags() & bit != 0
    }

    pub fn includes_opacity(self) -> bool {
        self.includes_flag(1)
    }

    pub fn includes_background_color(self) -> bool {
        self.includes_flag(2)
    }

    pub fn includes_color(self) -> bool {
        self.includes_flag(4)
    }

    pub fn includes_border_top_color(self) -> bool {
        self.includes_flag(8)
    }

    pub fn includes_border_bottom_color(self) -> bool {
        self.includes_flag(16)
    }

    pub fn includes_border_left_color(self) -> bool {
        self.includes_flag(32)
    }

    pub fn includes_border_right_color(self) -> bool {
        self.includes_flag(64)
    }

    pub fn includes_border_top_width(self) -> bool {
        self.includes_flag(2048)
    }

    pub fn includes_border_bottom_width(self) -> bool {
        self.includes_flag(4096)
    }

    pub fn includes_border_left_width(self) -> bool {
        self.includes_flag(8192)
    }

    pub fn includes_border_right_width(self) -> bool {
        self.includes_flag(16384)
    }

    pub fn includes_border_radius(self) -> bool {
        self.includes_flag(128)
    }

    pub fn includes_transform(self) -> bool {
        self.includes_flag(256)
    }

    pub fn includes_background_position(self) -> bool {
        self.includes_flag(512)
    }

    pub fn includes_box_shadow(self) -> bool {
        self.includes_flag(1024)
    }

    pub fn includes_background_image(self) -> bool {
        self.includes_flag(32768)
    }

    pub fn includes_border_top_style(self) -> bool {
        self.includes_flag(65536)
    }

    pub fn includes_border_bottom_style(self) -> bool {
        self.includes_flag(131072)
    }

    pub fn includes_border_left_style(self) -> bool {
        self.includes_flag(262144)
    }

    pub fn includes_border_right_style(self) -> bool {
        self.includes_flag(524288)
    }

    /// Every longhand the retained transition clock may drive (harvest H2).
    /// The `border-radius` flag covers its four corner longhands.
    pub const TRANSITIONABLE: &'static [crate::PropertyId] = &[
        crate::PropertyId::Opacity,
        crate::PropertyId::BackgroundColor,
        crate::PropertyId::Color,
        crate::PropertyId::BorderTopColor,
        crate::PropertyId::BorderBottomColor,
        crate::PropertyId::BorderLeftColor,
        crate::PropertyId::BorderRightColor,
        crate::PropertyId::BorderTopWidth,
        crate::PropertyId::BorderBottomWidth,
        crate::PropertyId::BorderLeftWidth,
        crate::PropertyId::BorderRightWidth,
        crate::PropertyId::BorderTopStyle,
        crate::PropertyId::BorderBottomStyle,
        crate::PropertyId::BorderLeftStyle,
        crate::PropertyId::BorderRightStyle,
        crate::PropertyId::BorderTopLeftRadius,
        crate::PropertyId::BorderTopRightRadius,
        crate::PropertyId::BorderBottomRightRadius,
        crate::PropertyId::BorderBottomLeftRadius,
        crate::PropertyId::Transform,
        crate::PropertyId::BackgroundPosition,
        crate::PropertyId::BoxShadow,
        crate::PropertyId::BackgroundImage,
        crate::PropertyId::BackgroundRepeat,
    ];

    /// Whether this transition-property value accepts one longhand
    /// (harvest H2: the generic form of the `includes_*` family).
    pub fn includes_property(self, property: crate::PropertyId) -> bool {
        use crate::PropertyId as P;
        match property {
            P::Opacity => self.includes_opacity(),
            P::BackgroundColor => self.includes_background_color(),
            P::Color => self.includes_color(),
            P::BorderTopColor => self.includes_border_top_color(),
            P::BorderBottomColor => self.includes_border_bottom_color(),
            P::BorderLeftColor => self.includes_border_left_color(),
            P::BorderRightColor => self.includes_border_right_color(),
            P::BorderTopWidth => self.includes_border_top_width(),
            P::BorderBottomWidth => self.includes_border_bottom_width(),
            P::BorderLeftWidth => self.includes_border_left_width(),
            P::BorderRightWidth => self.includes_border_right_width(),
            P::BorderTopStyle => self.includes_border_top_style(),
            P::BorderBottomStyle => self.includes_border_bottom_style(),
            P::BorderLeftStyle => self.includes_border_left_style(),
            P::BorderRightStyle => self.includes_border_right_style(),
            P::BorderTopLeftRadius
            | P::BorderTopRightRadius
            | P::BorderBottomRightRadius
            | P::BorderBottomLeftRadius => self.includes_border_radius(),
            P::Transform => self.includes_transform(),
            P::BackgroundPosition => self.includes_background_position(),
            P::BoxShadow => self.includes_box_shadow(),
            P::BackgroundImage => self.includes_background_image(),
            P::BackgroundRepeat => self.includes_background_repeat(),
            _ => false,
        }
    }

    pub fn includes_background_repeat(self) -> bool {
        self.includes_flag(1048576)
    }

    fn flags(self) -> u32 {
        match self {
            Self::All | Self::None => 0,
            Self::Opacity => 1,
            Self::BackgroundColor => 2,
            Self::Color => 4,
            Self::BorderTopColor => 8,
            Self::BorderBottomColor => 16,
            Self::BorderLeftColor => 32,
            Self::BorderRightColor => 64,
            Self::BorderTopWidth => 2048,
            Self::BorderBottomWidth => 4096,
            Self::BorderLeftWidth => 8192,
            Self::BorderRightWidth => 16384,
            Self::BorderRadius => 128,
            Self::Transform => 256,
            Self::BackgroundPosition => 512,
            Self::BoxShadow => 1024,
            Self::BackgroundImage => 32768,
            Self::BorderTopStyle => 65536,
            Self::BorderBottomStyle => 131072,
            Self::BorderLeftStyle => 262144,
            Self::BorderRightStyle => 524288,
            Self::BackgroundRepeat => 1048576,
            Self::List(flags) => flags,
            Self::OpacityAndBackgroundColor => 3,
            Self::OpacityAndColor => 5,
            Self::BackgroundColorAndColor => 6,
            Self::OpacityAndBackgroundColorAndColor => 7,
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => Self::All,
            (Self::None, value) | (value, Self::None) => value,
            (left, right) if left == right => left,
            (left, right) => Self::from_flags(left.flags() | right.flags()).unwrap_or(Self::All),
        }
    }
}

keyword_value! {
    /// Timing functions used by the first keyframe animation gate.
    pub enum TimingFunction {
        Linear => "linear",
        Ease => "ease",
        EaseIn => "ease-in",
        EaseOut => "ease-out",
        EaseInOut => "ease-in-out",
    }
}

impl TimingFunction {
    pub fn sample(self, progress: f32) -> f32 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => progress,
            Self::Ease => cubic_bezier(progress, 0.25, 0.1, 0.25, 1.0),
            Self::EaseIn => cubic_bezier(progress, 0.42, 0.0, 1.0, 1.0),
            Self::EaseOut => cubic_bezier(progress, 0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => cubic_bezier(progress, 0.42, 0.0, 0.58, 1.0),
        }
    }
}

fn cubic_bezier(progress: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let middle = (low + high) * 0.5;
        if bezier_axis(middle, x1, x2) < progress {
            low = middle;
        } else {
            high = middle;
        }
    }
    bezier_axis((low + high) * 0.5, y1, y2)
}

fn bezier_axis(t: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}

keyword_value! {
    /// CSS border line style.
    pub enum BorderStyle {
        None => "none",
        Hidden => "hidden",
        Dotted => "dotted",
        Dashed => "dashed",
        Solid => "solid",
        Double => "double",
        Groove => "groove",
        Ridge => "ridge",
        Inset => "inset",
        Outset => "outset",
    }
}

impl BorderStyle {
    /// Border styles are discrete in CSS. The bounded retained transition
    /// switches to the target at the midpoint of the clock.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        if progress.clamp(0.0, 1.0) < 0.5 {
            self
        } else {
            other
        }
    }
}

keyword_value! {
    /// Display keywords required by the Cambium lane and baseline UA sheet.
    pub enum Display {
        None => "none",
        Inline => "inline",
        Block => "block",
        InlineBlock => "inline-block",
        Flex => "flex",
        Grid => "grid",
        Table => "table",
        TableRowGroup => "table-row-group",
        TableRow => "table-row",
        TableCell => "table-cell",
        TableCaption => "table-caption",
    }
}

keyword_value! {
    /// Floating directions consumed by Taffy's bounded block float lane.
    pub enum Float {
        None => "none",
        Left => "left",
        Right => "right",
    }
}

keyword_value! {
    /// CSS box sizing mode used by the layout adapter.
    pub enum BoxSizing {
        ContentBox => "content-box",
        BorderBox => "border-box",
    }
}

keyword_value! {
    /// Axes exposed as query-container size bases.
    pub enum ContainerType {
        Normal => "normal",
        Size => "size",
        InlineSize => "inline-size",
    }
}

/// Names by which descendant `@container` rules can select this query
/// container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainerName {
    None,
    Names(Vec<String>),
}

impl ContainerName {
    pub fn contains(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::Names(names) => names.iter().any(|candidate| candidate == name),
        }
    }

    pub fn names(&self) -> &[String] {
        match self {
            Self::None => &[],
            Self::Names(names) => names,
        }
    }
}

impl FromStr for ContainerName {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let names = input
            .split_ascii_whitespace()
            .map(|name| {
                let lower = name.to_ascii_lowercase();
                let reserved = [
                    "none",
                    "default",
                    "initial",
                    "inherit",
                    "unset",
                    "revert",
                    "revert-layer",
                    "and",
                    "or",
                    "not",
                ];
                let valid = !reserved.contains(&lower.as_str())
                    && name.chars().next().is_some_and(|character| {
                        character.is_ascii_alphabetic() || character == '_'
                    })
                    && name.chars().all(|character| {
                        character.is_ascii_alphanumeric() || "-_".contains(character)
                    });
                valid
                    .then(|| name.to_owned())
                    .ok_or_else(|| ParseError::expected("none or one or more custom identifiers"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        (!names.is_empty())
            .then_some(Self::Names(names))
            .ok_or_else(|| ParseError::expected("none or one or more custom identifiers"))
    }
}

impl fmt::Display for ContainerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Names(names) => formatter.write_str(&names.join(" ")),
        }
    }
}

keyword_value! {
    /// Block-flow direction used to map logical viewport and container axes.
    pub enum WritingMode {
        HorizontalTb => "horizontal-tb",
        VerticalRl => "vertical-rl",
        VerticalLr => "vertical-lr",
        SidewaysRl => "sideways-rl",
        SidewaysLr => "sideways-lr",
    }
}

impl WritingMode {
    pub const fn is_vertical(self) -> bool {
        !matches!(self, Self::HorizontalTb)
    }
}

keyword_value! {
    /// Whether a box participates in hit testing and event dispatch.
    pub enum PointerEvents {
        Auto => "auto",
        None => "none",
    }
}

keyword_value! {
    /// Visibility state. Hidden boxes retain layout space but are not painted.
    pub enum Visibility {
        Visible => "visible",
        Hidden => "hidden",
        Collapse => "collapse",
    }
}

keyword_value! {
    /// Inline-axis text alignment keywords used by Parley.
    pub enum TextAlign {
        Start => "start",
        End => "end",
        Left => "left",
        Right => "right",
        Center => "center",
        Justify => "justify",
    }
}

keyword_value! {
    /// Flex and grid main/cross-axis alignment keywords.
    pub enum Alignment {
        Start => "start",
        End => "end",
        FlexStart => "flex-start",
        FlexEnd => "flex-end",
        Center => "center",
        Baseline => "baseline",
        Stretch => "stretch",
        SpaceBetween => "space-between",
        SpaceAround => "space-around",
        SpaceEvenly => "space-evenly",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Sub,
    Super,
    TextTop,
    TextBottom,
    Middle,
    Top,
    Bottom,
    Length(LengthPercentage),
}

impl FromStr for VerticalAlign {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "baseline" => Ok(Self::Baseline),
            "sub" => Ok(Self::Sub),
            "super" => Ok(Self::Super),
            "text-top" => Ok(Self::TextTop),
            "text-bottom" => Ok(Self::TextBottom),
            "middle" => Ok(Self::Middle),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            _ => input.parse().map(Self::Length),
        }
    }
}

impl fmt::Display for VerticalAlign {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Baseline => formatter.write_str("baseline"),
            Self::Sub => formatter.write_str("sub"),
            Self::Super => formatter.write_str("super"),
            Self::TextTop => formatter.write_str("text-top"),
            Self::TextBottom => formatter.write_str("text-bottom"),
            Self::Middle => formatter.write_str("middle"),
            Self::Top => formatter.write_str("top"),
            Self::Bottom => formatter.write_str("bottom"),
            Self::Length(value) => value.fmt(formatter),
        }
    }
}

keyword_value! {
    pub enum FlexDirection {
        Row => "row",
        RowReverse => "row-reverse",
        Column => "column",
        ColumnReverse => "column-reverse",
    }
}

keyword_value! {
    pub enum FlexWrap {
        NoWrap => "nowrap",
        Wrap => "wrap",
        WrapReverse => "wrap-reverse",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GridAutoFlow {
    Row,
    Column,
    RowDense,
    ColumnDense,
}

impl FromStr for GridAutoFlow {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "row" => Ok(Self::Row),
            "column" => Ok(Self::Column),
            "row dense" => Ok(Self::RowDense),
            "column dense" => Ok(Self::ColumnDense),
            _ => Err(ParseError::expected("grid-auto-flow keywords")),
        }
    }
}

impl fmt::Display for GridAutoFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Row => "row",
            Self::Column => "column",
            Self::RowDense => "row dense",
            Self::ColumnDense => "column dense",
        })
    }
}

keyword_value! {
    pub enum FontStyle {
        Normal => "normal",
        Italic => "italic",
        Oblique => "oblique",
    }
}

keyword_value! {
    pub enum ListStyleType {
        None => "none",
        Disc => "disc",
        Decimal => "decimal",
    }
}

keyword_value! {
    pub enum Overflow {
        Visible => "visible",
        Hidden => "hidden",
        Clip => "clip",
        Scroll => "scroll",
        Auto => "auto",
    }
}

keyword_value! {
    pub enum Position {
        Static => "static",
        Relative => "relative",
        Absolute => "absolute",
        Sticky => "sticky",
        Fixed => "fixed",
    }
}

keyword_value! {
    pub enum TextWrapMode {
        Wrap => "wrap",
        Nowrap => "nowrap",
    }
}

keyword_value! {
    pub enum WhiteSpaceCollapse {
        Collapse => "collapse",
        Discard => "discard",
        Preserve => "preserve",
        PreserveBreaks => "preserve-breaks",
        PreserveSpaces => "preserve-spaces",
        BreakSpaces => "break-spaces",
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BorderWidth {
    Thin,
    Medium,
    Thick,
    Length(Length),
}

impl BorderWidth {
    /// Interpolate the bounded line-width family used by the border paint
    /// lane. Fixed keyword widths participate in the px family; length
    /// endpoints interpolate only when their units match.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        match (self, other) {
            (Self::Length(from), Self::Length(to)) if from.unit == to.unit => {
                Self::Length(Length {
                    value: from.value + (to.value - from.value) * progress,
                    unit: from.unit,
                })
            },
            (from, to) => match (from.computed_px(), to.computed_px()) {
                (Some(from), Some(to)) => Self::Length(Length::px(from + (to - from) * progress)),
                _ if progress < 0.5 => from,
                _ => to,
            },
        }
    }

    fn computed_px(self) -> Option<f32> {
        match self {
            Self::Thin => Some(1.0),
            Self::Medium => Some(3.0),
            Self::Thick => Some(5.0),
            Self::Length(length) if length.unit == super::LengthUnit::Px => Some(length.value),
            _ => None,
        }
    }
}

impl FromStr for BorderWidth {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "thin" => Ok(Self::Thin),
            "medium" => Ok(Self::Medium),
            "thick" => Ok(Self::Thick),
            _ => input.parse::<Length>().map(Self::Length),
        }
    }
}

impl fmt::Display for BorderWidth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thin => formatter.write_str("thin"),
            Self::Medium => formatter.write_str("medium"),
            Self::Thick => formatter.write_str("thick"),
            Self::Length(length) => length.fmt(formatter),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFamily {
    UserAgentDefault,
    SystemUi,
    Named(Box<str>),
}

impl FromStr for FontFamily {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("system-ui") {
            return Ok(Self::SystemUi);
        }
        if input.eq_ignore_ascii_case("depends-on-user-agent") {
            return Ok(Self::UserAgentDefault);
        }
        if input.is_empty() || input.contains(',') {
            return Err(ParseError::expected("one seed font family"));
        }
        let unquoted = input
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(input);
        Ok(Self::Named(unquoted.into()))
    }
}

impl fmt::Display for FontFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserAgentDefault => formatter.write_str("depends-on-user-agent"),
            Self::SystemUi => formatter.write_str("system-ui"),
            Self::Named(name) if name.contains(char::is_whitespace) => {
                write!(formatter, "\"{name}\"")
            },
            Self::Named(name) => formatter.write_str(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontSize {
    Medium,
    Value(LengthPercentage),
}

impl FromStr for FontSize {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.trim().eq_ignore_ascii_case("medium") {
            Ok(Self::Medium)
        } else {
            input.parse::<LengthPercentage>().map(Self::Value)
        }
    }
}

impl fmt::Display for FontSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Medium => formatter.write_str("medium"),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontWeight {
    Normal,
    Bold,
    Bolder,
    Lighter,
    Number(u16),
}

impl FromStr for FontWeight {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "bold" => Ok(Self::Bold),
            "bolder" => Ok(Self::Bolder),
            "lighter" => Ok(Self::Lighter),
            number => number
                .parse::<u16>()
                .ok()
                .filter(|number| (1..=1000).contains(number))
                .map(Self::Number)
                .ok_or_else(|| ParseError::expected("a font weight from 1 through 1000")),
        }
    }
}

impl fmt::Display for FontWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => formatter.write_str("normal"),
            Self::Bold => formatter.write_str("bold"),
            Self::Bolder => formatter.write_str("bolder"),
            Self::Lighter => formatter.write_str("lighter"),
            Self::Number(number) => number.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Auto,
    None,
    MinContent,
    MaxContent,
    FitContent(LengthPercentage),
    Value(LengthPercentage),
}

impl FromStr for Size {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        if input.eq_ignore_ascii_case("min-content") {
            return Ok(Self::MinContent);
        }
        if input.eq_ignore_ascii_case("max-content") {
            return Ok(Self::MaxContent);
        }
        if input.len() > 13
            && input[..12].eq_ignore_ascii_case("fit-content(")
            && input.ends_with(')')
        {
            return input[12..input.len() - 1]
                .parse::<LengthPercentage>()
                .map(Self::FitContent);
        }
        input.parse::<LengthPercentage>().map(Self::Value)
    }
}

impl fmt::Display for Size {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::None => formatter.write_str("none"),
            Self::MinContent => formatter.write_str("min-content"),
            Self::MaxContent => formatter.write_str("max-content"),
            Self::FitContent(value) => write!(formatter, "fit-content({value})"),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridTrack {
    Auto,
    MinContent,
    MaxContent,
    Length(Length),
    Percent(f32),
    Fr(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum GridTemplate {
    None,
    Tracks(Vec<GridTrack>),
}

impl FromStr for GridTemplate {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let mut tracks = Vec::new();
        for component in input.split_ascii_whitespace() {
            let track = if component.eq_ignore_ascii_case("auto") {
                GridTrack::Auto
            } else if component.eq_ignore_ascii_case("min-content") {
                GridTrack::MinContent
            } else if component.eq_ignore_ascii_case("max-content") {
                GridTrack::MaxContent
            } else if let Some(value) = component.strip_suffix("fr") {
                GridTrack::Fr(parse_non_negative(value)?)
            } else if let Some(value) = component.strip_suffix('%') {
                GridTrack::Percent(parse_non_negative(value)? / 100.0)
            } else {
                GridTrack::Length(
                    component
                        .parse::<Length>()
                        .map_err(|_| ParseError::expected("grid track sizes"))?,
                )
            };
            tracks.push(track);
        }
        if tracks.is_empty() {
            Err(ParseError::expected("one or more grid tracks"))
        } else {
            Ok(Self::Tracks(tracks))
        }
    }
}

impl fmt::Display for GridTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Tracks(tracks) => {
                for (index, track) in tracks.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" ")?;
                    }
                    track.fmt(formatter)?;
                }
                Ok(())
            },
        }
    }
}

impl fmt::Display for GridTrack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::MinContent => formatter.write_str("min-content"),
            Self::MaxContent => formatter.write_str("max-content"),
            Self::Length(value) => value.fmt(formatter),
            Self::Percent(value) => write!(formatter, "{}%", format_number(*value * 100.0)),
            Self::Fr(value) => write!(formatter, "{}fr", format_number(*value)),
        }
    }
}

fn parse_non_negative(input: &str) -> Result<f32, ParseError> {
    input
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| ParseError::expected("a non-negative grid track number"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GridPlacement {
    Auto,
    Line(i16),
    Span(u16),
}

impl FromStr for GridPlacement {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if let Some(span) = input.strip_prefix("span ") {
            return span
                .parse::<u16>()
                .ok()
                .filter(|value| *value > 0)
                .map(Self::Span)
                .ok_or_else(|| ParseError::expected("a positive grid span"));
        }
        input
            .parse::<i16>()
            .map(Self::Line)
            .map_err(|_| ParseError::expected("auto, span, or a grid line number"))
    }
}

impl fmt::Display for GridPlacement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Line(value) => value.fmt(formatter),
            Self::Span(value) => write!(formatter, "span {value}"),
        }
    }
}

/// CSS `aspect-ratio`, represented as width divided by height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AspectRatio {
    Auto,
    Ratio(f32),
}

impl FromStr for AspectRatio {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        let (width, height) = input
            .split_once('/')
            .map_or((input, "1"), |(width, height)| {
                (width.trim(), height.trim())
            });
        let width = width
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
            .ok_or_else(|| ParseError::expected("a positive aspect-ratio"))?;
        let height = height
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
            .ok_or_else(|| ParseError::expected("a positive aspect-ratio"))?;
        Ok(Self::Ratio(width / height))
    }
}

impl fmt::Display for AspectRatio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Ratio(value) => formatter.write_str(&format_number(*value)),
        }
    }
}

macro_rules! auto_length_percentage {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub enum $name {
            Auto,
            Value(LengthPercentage),
        }

        impl FromStr for $name {
            type Err = ParseError;

            fn from_str(input: &str) -> Result<Self, Self::Err> {
                if input.trim().eq_ignore_ascii_case("auto") {
                    Ok(Self::Auto)
                } else {
                    input.parse::<LengthPercentage>().map(Self::Value)
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::Auto => formatter.write_str("auto"),
                    Self::Value(value) => value.fmt(formatter),
                }
            }
        }
    };
}

auto_length_percentage!(Inset);
auto_length_percentage!(Margin);

/// A non-negative border corner radius component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radius(pub LengthPercentage);

impl Radius {
    pub const ZERO: Self = Self(LengthPercentage::ZERO);

    /// Interpolate the bounded radius forms used by the retained paint lane.
    /// Zero and a concrete length/percentage share the same scalar family;
    /// mixed non-zero units stay discrete until the broader value ratchet.
    pub fn interpolate(self, other: Self, progress: f32) -> Self {
        Self(self.0.interpolate(other.0, progress))
    }
}

impl FromStr for Radius {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.parse::<LengthPercentage>()?;
        let negative = match value {
            LengthPercentage::Zero => false,
            LengthPercentage::Length(length) => length.value < 0.0,
            LengthPercentage::Percentage(value) => value < 0.0,
            LengthPercentage::Calc(calc) => calc.px < 0.0 || calc.em < 0.0 || calc.rem < 0.0,
            LengthPercentage::Math(_) => false,
        };
        if negative {
            return Err(ParseError::expected("a non-negative border radius"));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Radius {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A non-negative flex/grid gap component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap(pub LengthPercentage);

impl Gap {
    pub const ZERO: Self = Self(LengthPercentage::ZERO);
}

impl FromStr for Gap {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.parse::<LengthPercentage>()?;
        let negative = match value {
            LengthPercentage::Zero => false,
            LengthPercentage::Length(length) => length.value < 0.0,
            LengthPercentage::Percentage(value) => value < 0.0,
            LengthPercentage::Calc(calc) => calc.px < 0.0 || calc.em < 0.0 || calc.rem < 0.0,
            LengthPercentage::Math(_) => false,
        };
        if negative {
            return Err(ParseError::expected("a non-negative gap"));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Gap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexFactor(f32);

impl FlexFactor {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    pub const fn value(self) -> f32 {
        self.0
    }
}

impl FromStr for FlexFactor {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        input
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(Self)
            .ok_or_else(|| ParseError::expected("a non-negative flex factor"))
    }
}

impl fmt::Display for FlexFactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_number(self.0))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Order(i32);

impl Order {
    pub const ZERO: Self = Self(0);

    pub const fn value(self) -> i32 {
        self.0
    }
}

impl FromStr for Order {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        input
            .trim()
            .parse::<i32>()
            .map(Self)
            .map_err(|_| ParseError::expected("an integer order"))
    }
}

impl fmt::Display for Order {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A CSS spacing value, with `normal` represented explicitly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    Normal,
    Length(LengthPercentage),
}

impl FromStr for Spacing {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.trim().eq_ignore_ascii_case("normal") {
            Ok(Self::Normal)
        } else {
            input
                .parse::<LengthPercentage>()
                .map(Self::Length)
                .map_err(|_| ParseError::expected("normal or a length"))
        }
    }
}

impl fmt::Display for Spacing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => formatter.write_str("normal"),
            Self::Length(length) => length.fmt(formatter),
        }
    }
}

pub type TextDecorationColor = super::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Value(LengthPercentage),
}

impl FromStr for LineHeight {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("normal") {
            return Ok(Self::Normal);
        }
        if let Ok(number) = input.parse::<f32>()
            && number.is_finite()
            && number >= 0.0
        {
            return Ok(Self::Number(number));
        }
        input.parse::<LengthPercentage>().map(Self::Value)
    }
}

impl fmt::Display for LineHeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal => formatter.write_str("normal"),
            Self::Number(number) => formatter.write_str(&format_number(*number)),
            Self::Value(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opacity(f32);

impl Opacity {
    pub const ONE: Self = Self(1.0);

    pub const fn from_value(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    pub const fn value(self) -> f32 {
        self.0
    }
}

impl FromStr for Opacity {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let value = if let Some(percentage) = input.strip_suffix('%') {
            percentage.trim().parse::<f32>().map(|value| value / 100.0)
        } else {
            input.parse::<f32>()
        }
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ParseError::expected("a finite opacity number or percentage"))?;
        Ok(Self(value.clamp(0.0, 1.0)))
    }
}

impl fmt::Display for Opacity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_number(self.0))
    }
}

/// The bounded 2D individual `rotate` property.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rotate {
    None,
    Angle(f32),
}

impl Rotate {
    pub const fn radians(self) -> Option<f32> {
        match self {
            Self::None => None,
            Self::Angle(value) => Some(value),
        }
    }
}

impl FromStr for Rotate {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        parse_angle(input)
            .or_else(|_| super::calc::parse_angle(input))
            .map(Self::Angle)
    }
}

impl fmt::Display for Rotate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Angle(value) => write!(formatter, "{}rad", format_number(*value)),
        }
    }
}

/// The bounded uniform individual `scale` property.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    None,
    Uniform(f32),
}

impl Scale {
    pub const fn factor(self) -> Option<f32> {
        match self {
            Self::None => None,
            Self::Uniform(value) => Some(value),
        }
    }
}

impl FromStr for Scale {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let value = input
            .strip_suffix('%')
            .and_then(|value| value.trim().parse::<f32>().ok())
            .map(|value| value / 100.0)
            .or_else(|| input.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .map(Ok)
            .unwrap_or_else(|| super::calc::parse_number(input))?;
        Ok(Self::Uniform(value))
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Uniform(value) => formatter.write_str(&format_number(*value)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Transform {
    None,
    Functions(Vec<TransformFunction>),
}

/// A bounded single-layer CSS box shadow.
#[derive(Clone, Debug, PartialEq)]
pub enum BoxShadow {
    None,
    Value(BoxShadowValue),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadowValue {
    pub inset: bool,
    pub offset_x: Length,
    pub offset_y: Length,
    pub blur_radius: Length,
    pub spread_radius: Length,
    pub color: Color,
}

impl BoxShadow {
    /// Interpolate the bounded single-shadow form used by the retained paint
    /// lane. Matching length units and inset mode are required; `none`, mixed
    /// units, and mode changes stay discrete until the shadow-list ratchet.
    pub fn interpolate(&self, other: &Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        let value = match (self, other) {
            (Self::Value(from), Self::Value(to)) if from.inset == to.inset => {
                interpolate_box_shadow_value(*from, *to, progress).map(Self::Value)
            },
            _ => None,
        };
        value.unwrap_or_else(|| {
            if progress < 0.5 {
                self.clone()
            } else {
                other.clone()
            }
        })
    }
}

fn interpolate_box_shadow_value(
    from: BoxShadowValue,
    to: BoxShadowValue,
    progress: f32,
) -> Option<BoxShadowValue> {
    Some(BoxShadowValue {
        inset: from.inset,
        offset_x: interpolate_length(from.offset_x, to.offset_x, progress)?,
        offset_y: interpolate_length(from.offset_y, to.offset_y, progress)?,
        blur_radius: interpolate_length(from.blur_radius, to.blur_radius, progress)?,
        spread_radius: interpolate_length(from.spread_radius, to.spread_radius, progress)?,
        color: from.color.interpolate(to.color, progress),
    })
}

impl FromStr for BoxShadow {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        let mut inset = false;
        let mut color = None;
        let mut lengths = Vec::new();
        for component in shadow_components(input) {
            if component.eq_ignore_ascii_case("inset") {
                if inset {
                    return Err(ParseError::expected("one inset box-shadow keyword"));
                }
                inset = true;
            } else if let Ok(value) = component.parse::<Color>() {
                if color.replace(value).is_some() {
                    return Err(ParseError::expected("one box-shadow color"));
                }
            } else if let Ok(value) = component.parse::<Length>() {
                lengths.push(value);
            } else {
                return Err(ParseError::expected("a bounded box-shadow component"));
            }
        }
        if !(2..=4).contains(&lengths.len()) {
            return Err(ParseError::expected("two through four box-shadow lengths"));
        }
        Ok(Self::Value(BoxShadowValue {
            inset,
            offset_x: lengths[0],
            offset_y: lengths[1],
            blur_radius: lengths.get(2).copied().unwrap_or(Length::ZERO),
            spread_radius: lengths.get(3).copied().unwrap_or(Length::ZERO),
            color: color.unwrap_or(Color::CurrentColor),
        }))
    }
}

impl fmt::Display for BoxShadow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Value(value) => {
                write!(
                    formatter,
                    "{} {} {} {} {}",
                    value.offset_x,
                    value.offset_y,
                    value.blur_radius,
                    value.spread_radius,
                    value.color
                )?;
                if value.inset {
                    formatter.write_str(" inset")?;
                }
                Ok(())
            },
        }
    }
}

fn shadow_components(input: &str) -> Vec<&str> {
    let mut components = Vec::new();
    let mut start = None;
    let mut depth = 0_u32;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => {
                start.get_or_insert(index);
                depth += 1;
            },
            ')' => depth = depth.saturating_sub(1),
            _ if ch.is_ascii_whitespace() && depth == 0 => {
                if let Some(offset) = start.take() {
                    components.push(&input[offset..index]);
                }
            },
            _ => {
                start.get_or_insert(index);
            },
        }
    }
    if let Some(offset) = start {
        components.push(&input[offset..]);
    }
    components
}

impl Transform {
    pub fn functions(&self) -> Option<&[TransformFunction]> {
        match self {
            Self::None => None,
            Self::Functions(functions) => Some(functions),
        }
    }

    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Interpolate matching transform primitives directly, then normalize any
    /// mismatched suffix (including `none`) through a decomposed 2D matrix.
    pub fn interpolate(&self, other: &Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        if matches!((self, other), (Self::None, Self::None)) {
            return Self::None;
        }

        let from = self.functions().unwrap_or(&[]);
        let to = other.functions().unwrap_or(&[]);
        let mut functions = Vec::new();
        let mut prefix = 0;
        while let (Some(from), Some(to)) = (from.get(prefix), to.get(prefix)) {
            let Some(value) = interpolate_transform_function(*from, *to, progress) else {
                break;
            };
            functions.push(value);
            prefix += 1;
        }
        if prefix == from.len() && prefix == to.len() {
            return Self::Functions(functions);
        }

        let from_matrix = Matrix2D::from_absolute_functions(&from[prefix..], 16.0);
        let to_matrix = Matrix2D::from_absolute_functions(&to[prefix..], 16.0);
        match from_matrix
            .zip(to_matrix)
            .and_then(|(from, to)| from.interpolate(to, progress))
        {
            Some(matrix) => {
                functions.push(TransformFunction::Matrix(matrix));
                Self::Functions(functions)
            },
            None if progress < 0.5 => self.clone(),
            None => other.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformFunction {
    Translate(LengthPercentage, LengthPercentage),
    Scale(f32, f32),
    Rotate(f32),
    Skew(f32, f32),
    Matrix(Matrix2D),
}

fn interpolate_transform_function(
    from: TransformFunction,
    to: TransformFunction,
    progress: f32,
) -> Option<TransformFunction> {
    let scalar = |from: f32, to: f32| from + (to - from) * progress;
    Some(match (from, to) {
        (
            TransformFunction::Translate(from_x, from_y),
            TransformFunction::Translate(to_x, to_y),
        ) => TransformFunction::Translate(
            from_x.interpolate(to_x, progress),
            from_y.interpolate(to_y, progress),
        ),
        (TransformFunction::Scale(from_x, from_y), TransformFunction::Scale(to_x, to_y)) => {
            TransformFunction::Scale(scalar(from_x, to_x), scalar(from_y, to_y))
        },
        (TransformFunction::Rotate(from), TransformFunction::Rotate(to)) => {
            TransformFunction::Rotate(scalar(from, to))
        },
        (TransformFunction::Skew(from_x, from_y), TransformFunction::Skew(to_x, to_y)) => {
            TransformFunction::Skew(scalar(from_x, to_x), scalar(from_y, to_y))
        },
        (TransformFunction::Matrix(from), TransformFunction::Matrix(to)) => {
            TransformFunction::Matrix(from.interpolate(to, progress)?)
        },
        _ => return None,
    })
}

fn interpolate_length(from: Length, to: Length, progress: f32) -> Option<Length> {
    (from.unit == to.unit).then_some(Length {
        value: from.value + (to.value - from.value) * progress,
        unit: from.unit,
    })
}

impl FromStr for Transform {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }

        let mut functions = Vec::new();
        while !input.is_empty() {
            let open = input
                .find('(')
                .ok_or_else(|| ParseError::expected("a supported 2D transform function"))?;
            let name = input[..open].trim().to_ascii_lowercase();
            if name.is_empty() || name.split_ascii_whitespace().count() != 1 {
                return Err(ParseError::expected("a supported 2D transform function"));
            }
            let tail = &input[open + 1..];
            let close = tail
                .find(')')
                .ok_or_else(|| ParseError::expected("a closed 2D transform function"))?;
            let arguments = tail[..close]
                .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            functions.push(parse_transform_function(&name, &arguments)?);
            input = tail[close + 1..].trim_start();
        }
        if functions.is_empty() {
            Err(ParseError::expected("none or a 2D transform list"))
        } else {
            Ok(Self::Functions(functions))
        }
    }
}

fn parse_transform_function(
    name: &str,
    arguments: &[&str],
) -> Result<TransformFunction, ParseError> {
    let length_percentage = |value: &str| value.parse::<LengthPercentage>();
    let number = |value: &str| {
        value
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| ParseError::expected("a finite transform number"))
    };
    match (name, arguments) {
        ("translate", [x]) => Ok(TransformFunction::Translate(
            length_percentage(x)?,
            LengthPercentage::ZERO,
        )),
        ("translate", [x, y]) => Ok(TransformFunction::Translate(
            length_percentage(x)?,
            length_percentage(y)?,
        )),
        ("translatex", [x]) => Ok(TransformFunction::Translate(
            length_percentage(x)?,
            LengthPercentage::ZERO,
        )),
        ("translatey", [y]) => Ok(TransformFunction::Translate(
            LengthPercentage::ZERO,
            length_percentage(y)?,
        )),
        ("scale", [both]) => {
            let both = number(both)?;
            Ok(TransformFunction::Scale(both, both))
        },
        ("scale", [x, y]) => Ok(TransformFunction::Scale(number(x)?, number(y)?)),
        ("scalex", [x]) => Ok(TransformFunction::Scale(number(x)?, 1.0)),
        ("scaley", [y]) => Ok(TransformFunction::Scale(1.0, number(y)?)),
        ("rotate", [angle]) => Ok(TransformFunction::Rotate(parse_angle(angle)?)),
        ("skew", [x]) => Ok(TransformFunction::Skew(parse_angle(x)?, 0.0)),
        ("skew", [x, y]) => Ok(TransformFunction::Skew(parse_angle(x)?, parse_angle(y)?)),
        ("skewx", [x]) => Ok(TransformFunction::Skew(parse_angle(x)?, 0.0)),
        ("skewy", [y]) => Ok(TransformFunction::Skew(0.0, parse_angle(y)?)),
        ("matrix", [a, b, c, d, e, f]) => Ok(TransformFunction::Matrix(Matrix2D::new(
            number(a)?,
            number(b)?,
            number(c)?,
            number(d)?,
            number(e)?,
            number(f)?,
        ))),
        _ => Err(ParseError::expected(
            "translate, scale, rotate, skew, or matrix",
        )),
    }
}

fn parse_angle(input: &str) -> Result<f32, ParseError> {
    let lower = input.trim().to_ascii_lowercase();
    let (number, factor) = if let Some(value) = lower.strip_suffix("deg") {
        (value, std::f32::consts::PI / 180.0)
    } else if let Some(value) = lower.strip_suffix("grad") {
        (value, std::f32::consts::PI / 200.0)
    } else if let Some(value) = lower.strip_suffix("rad") {
        (value, 1.0)
    } else if let Some(value) = lower.strip_suffix("turn") {
        (value, std::f32::consts::TAU)
    } else if lower == "0" || lower == "+0" || lower == "-0" {
        ("0", 1.0)
    } else {
        return Err(ParseError::expected("a deg, rad, or turn angle"));
    };
    number
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value * factor)
        .ok_or_else(|| ParseError::expected("a finite angle"))
}

impl fmt::Display for Transform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Functions(functions) => {
                for (index, function) in functions.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" ")?;
                    }
                    function.fmt(formatter)?;
                }
                Ok(())
            },
        }
    }
}

impl fmt::Display for TransformFunction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Translate(x, y) => write!(formatter, "translate({x}, {y})"),
            Self::Scale(x, y) => write!(
                formatter,
                "scale({}, {})",
                format_number(*x),
                format_number(*y)
            ),
            Self::Rotate(radians) => write!(formatter, "rotate({}rad)", format_number(*radians)),
            Self::Skew(x, y) => write!(
                formatter,
                "skew({}rad, {}rad)",
                format_number(*x),
                format_number(*y)
            ),
            Self::Matrix(matrix) => matrix.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Padding(pub LengthPercentage);

impl Padding {
    pub const ZERO: Self = Self(LengthPercentage::ZERO);
}

impl FromStr for Padding {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.parse::<LengthPercentage>()?;
        let negative = match value {
            LengthPercentage::Zero => false,
            LengthPercentage::Length(length) => length.value < 0.0,
            LengthPercentage::Percentage(value) => value < 0.0,
            LengthPercentage::Calc(_) => false,
            LengthPercentage::Math(_) => false,
        };
        if negative {
            return Err(ParseError::expected("a non-negative padding"));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Padding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextDecorationLine(u8);

impl TextDecorationLine {
    pub const NONE: Self = Self(0);
    const UNDERLINE: u8 = 1 << 0;
    const OVERLINE: u8 = 1 << 1;
    const LINE_THROUGH: u8 = 1 << 2;
    const BLINK: u8 = 1 << 3;

    pub const fn contains_underline(self) -> bool {
        self.0 & Self::UNDERLINE != 0
    }
}

impl FromStr for TextDecorationLine {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("none") {
            return Ok(Self::NONE);
        }
        let mut flags = 0;
        for keyword in input.split_ascii_whitespace() {
            let flag = match keyword.to_ascii_lowercase().as_str() {
                "underline" => Self::UNDERLINE,
                "overline" => Self::OVERLINE,
                "line-through" => Self::LINE_THROUGH,
                "blink" => Self::BLINK,
                _ => return Err(ParseError::expected("text-decoration-line keywords")),
            };
            if flags & flag != 0 {
                return Err(ParseError::expected("unique text-decoration-line keywords"));
            }
            flags |= flag;
        }
        if flags == 0 {
            return Err(ParseError::expected("text-decoration-line keywords"));
        }
        Ok(Self(flags))
    }
}

impl fmt::Display for TextDecorationLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::NONE {
            return formatter.write_str("none");
        }
        let mut first = true;
        for (flag, name) in [
            (Self::UNDERLINE, "underline"),
            (Self::OVERLINE, "overline"),
            (Self::LINE_THROUGH, "line-through"),
            (Self::BLINK, "blink"),
        ] {
            if self.0 & flag == 0 {
                continue;
            }
            if !first {
                formatter.write_str(" ")?;
            }
            formatter.write_str(name)?;
            first = false;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZIndex {
    Auto,
    Integer(i32),
}

impl FromStr for ZIndex {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.trim().eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            let integer = input
                .trim()
                .parse::<i32>()
                .ok()
                .or_else(|| {
                    input.contains('(').then(|| super::calc::parse_number(input).ok()).flatten().and_then(|value| {
                        let rounded = (value + 0.5).floor();
                        (rounded >= i32::MIN as f32 && rounded <= i32::MAX as f32)
                            .then_some(rounded as i32)
                    })
                })
                .ok_or_else(|| ParseError::expected("auto or an integer"))?;
            Ok(Self::Integer(integer))
        }
    }
}

impl fmt::Display for ZIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Integer(value) => value.fmt(formatter),
        }
    }
}
