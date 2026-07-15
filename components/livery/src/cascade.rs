//! DOM-neutral declaration parsing and cascade ordering.

use std::{cmp::Ordering, fmt};

use crate::values::{
    AnimationName, BorderStyle, BorderWidth, Color, Duration, Margin, Padding, Radius,
    TimingFunction, TransitionProperty,
};
use crate::{ComputedValues, PropertyId, PropertyValue, ShorthandId};

/// A parsed longhand value, including the CSS-wide keywords supported by the
/// first lane.
#[derive(Clone, Debug, PartialEq)]
pub enum DeclaredValue {
    Value(PropertyValue),
    Initial,
    Inherit,
    Unset,
}

impl DeclaredValue {
    fn parse(property: PropertyId, input: &str) -> Result<Self, crate::values::ParseError> {
        match input.trim().to_ascii_lowercase().as_str() {
            "initial" => Ok(Self::Initial),
            "inherit" => Ok(Self::Inherit),
            "unset" => Ok(Self::Unset),
            _ => PropertyValue::parse(property, input).map(Self::Value),
        }
    }
}

/// One parsed longhand declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub property: PropertyId,
    pub value: DeclaredValue,
    pub important: bool,
}

/// Why an authored declaration was ignored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclarationErrorKind {
    UnknownProperty,
    InvalidValue,
    MalformedDeclaration,
}

/// A non-fatal declaration parse diagnostic. CSS drops the declaration and
/// continues parsing the block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclarationError {
    pub name: String,
    pub value: String,
    pub kind: DeclarationErrorKind,
}

/// Parsed declarations plus the declarations CSS error recovery discarded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeclarationBlock {
    pub declarations: Vec<Declaration>,
    pub errors: Vec<DeclarationError>,
}

fn without_comments(css: &str) -> String {
    let mut clean = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    let mut in_comment = false;
    while let Some(ch) = chars.next() {
        if in_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_comment = false;
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_comment = true;
        } else {
            clean.push(ch);
        }
    }
    clean
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                parts.push(&input[start..index]);
                start = index + ch.len_utf8();
            },
            _ => {},
        }
    }
    parts.push(&input[start..]);
    parts
}

fn split_components(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = None;
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                start.get_or_insert(index);
                quote = Some(ch);
            },
            '(' => {
                start.get_or_insert(index);
                depth += 1;
            },
            ')' => depth = depth.saturating_sub(1),
            _ if ch.is_ascii_whitespace() && depth == 0 => {
                if let Some(part_start) = start.take() {
                    parts.push(&input[part_start..index]);
                }
            },
            _ => {
                start.get_or_insert(index);
            },
        }
    }
    if let Some(part_start) = start {
        parts.push(&input[part_start..]);
    }
    parts
}

fn strip_important(value: &str) -> (&str, bool) {
    let trimmed = value.trim();
    let Some(bang) = trimmed.rfind('!') else {
        return (trimmed, false);
    };
    if trimmed[bang + 1..].trim().eq_ignore_ascii_case("important") {
        (trimmed[..bang].trim_end(), true)
    } else {
        (trimmed, false)
    }
}

fn push_longhand(block: &mut DeclarationBlock, name: &str, value: &str, important: bool) -> bool {
    let Some(property) = PropertyId::from_css_name(name) else {
        return false;
    };
    match DeclaredValue::parse(property, value) {
        Ok(value) => block.declarations.push(Declaration {
            property,
            value,
            important,
        }),
        Err(_) => block.errors.push(DeclarationError {
            name: name.to_owned(),
            value: value.to_owned(),
            kind: DeclarationErrorKind::InvalidValue,
        }),
    }
    true
}

fn box_sides<T: Clone>(values: &[T]) -> Option<[T; 4]> {
    match values {
        [all] => Some([all.clone(), all.clone(), all.clone(), all.clone()]),
        [vertical, horizontal] => Some([
            vertical.clone(),
            horizontal.clone(),
            vertical.clone(),
            horizontal.clone(),
        ]),
        [top, horizontal, bottom] => Some([
            top.clone(),
            horizontal.clone(),
            bottom.clone(),
            horizontal.clone(),
        ]),
        [top, right, bottom, left] => {
            Some([top.clone(), right.clone(), bottom.clone(), left.clone()])
        },
        _ => None,
    }
}

fn expand_box_shorthand(
    block: &mut DeclarationBlock,
    shorthand: ShorthandId,
    value: &str,
    important: bool,
) -> bool {
    let parsed = match shorthand {
        ShorthandId::Margin => split_components(value)
            .into_iter()
            .map(str::parse::<Margin>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .and_then(|values| box_sides(&values))
            .map(|values| values.map(|value| DeclaredValue::Value(PropertyValue::Margin(value)))),
        ShorthandId::Padding => split_components(value)
            .into_iter()
            .map(str::parse::<Padding>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .and_then(|values| box_sides(&values))
            .map(|values| values.map(|value| DeclaredValue::Value(PropertyValue::Padding(value)))),
        ShorthandId::BorderRadius => split_components(value)
            .into_iter()
            .map(str::parse::<Radius>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .and_then(|values| box_sides(&values))
            .map(|values| values.map(|value| DeclaredValue::Value(PropertyValue::Radius(value)))),
        ShorthandId::Gap => split_components(value)
            .into_iter()
            .map(str::parse::<crate::values::Gap>)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .and_then(|values| box_sides(&values))
            .map(|values| values.map(|value| DeclaredValue::Value(PropertyValue::Gap(value)))),
        _ => return false,
    };
    let Some(values) = parsed else {
        block.errors.push(DeclarationError {
            name: shorthand.metadata().name.to_owned(),
            value: value.to_owned(),
            kind: DeclarationErrorKind::InvalidValue,
        });
        return true;
    };
    for (&property, value) in shorthand.metadata().longhands.iter().zip(values) {
        block.declarations.push(Declaration {
            property,
            value,
            important,
        });
    }
    true
}

fn expand_transition(block: &mut DeclarationBlock, value: &str, important: bool) {
    let mut property = None;
    let mut duration = None;
    for component in split_components(value) {
        if property.is_none()
            && let Ok(parsed) = component.parse::<TransitionProperty>()
        {
            property = Some(parsed);
        } else if duration.is_none()
            && let Ok(parsed) = component.parse::<Duration>()
        {
            duration = Some(parsed);
        } else {
            block.errors.push(DeclarationError {
                name: "transition".to_owned(),
                value: value.to_owned(),
                kind: DeclarationErrorKind::InvalidValue,
            });
            return;
        }
    }
    let Some(duration) = duration else {
        block.errors.push(DeclarationError {
            name: "transition".to_owned(),
            value: value.to_owned(),
            kind: DeclarationErrorKind::InvalidValue,
        });
        return;
    };
    block.declarations.push(Declaration {
        property: PropertyId::TransitionProperty,
        value: DeclaredValue::Value(PropertyValue::TransitionProperty(
            property.unwrap_or(TransitionProperty::All),
        )),
        important,
    });
    block.declarations.push(Declaration {
        property: PropertyId::TransitionDuration,
        value: DeclaredValue::Value(PropertyValue::Duration(duration)),
        important,
    });
}

fn expand_animation(block: &mut DeclarationBlock, value: &str, important: bool) {
    let mut name = None;
    let mut duration = None;
    let mut timing = None;
    for component in split_components(value) {
        if duration.is_none()
            && let Ok(parsed) = component.parse::<Duration>()
        {
            duration = Some(parsed);
        } else if timing.is_none()
            && let Ok(parsed) = component.parse::<TimingFunction>()
        {
            timing = Some(parsed);
        } else if name.is_none()
            && let Ok(parsed) = component.parse::<AnimationName>()
        {
            name = Some(parsed);
        } else {
            block.errors.push(DeclarationError {
                name: "animation".to_owned(),
                value: value.to_owned(),
                kind: DeclarationErrorKind::InvalidValue,
            });
            return;
        }
    }
    let Some(duration) = duration else {
        block.errors.push(DeclarationError {
            name: "animation".to_owned(),
            value: value.to_owned(),
            kind: DeclarationErrorKind::InvalidValue,
        });
        return;
    };
    let push = |block: &mut DeclarationBlock, property, value| {
        block.declarations.push(Declaration {
            property,
            value: DeclaredValue::Value(value),
            important,
        });
    };
    push(
        block,
        PropertyId::AnimationName,
        PropertyValue::AnimationName(name.unwrap_or(AnimationName::None)),
    );
    push(
        block,
        PropertyId::AnimationDuration,
        PropertyValue::Duration(duration),
    );
    push(
        block,
        PropertyId::AnimationTimingFunction,
        PropertyValue::TimingFunction(timing.unwrap_or(TimingFunction::Linear)),
    );
}

fn expand_border(block: &mut DeclarationBlock, value: &str, important: bool) {
    let mut width = None;
    let mut style = None;
    let mut color = None;
    for component in split_components(value) {
        if width.is_none() {
            width = component.parse::<BorderWidth>().ok();
            if width.is_some() {
                continue;
            }
        }
        if style.is_none() {
            style = component.parse::<BorderStyle>().ok();
            if style.is_some() {
                continue;
            }
        }
        if color.is_none() {
            color = component.parse::<Color>().ok();
            if color.is_some() {
                continue;
            }
        }
        block.errors.push(DeclarationError {
            name: "border".to_owned(),
            value: value.to_owned(),
            kind: DeclarationErrorKind::InvalidValue,
        });
        return;
    }
    let width = width.unwrap_or(BorderWidth::Medium);
    let style = style.unwrap_or(BorderStyle::None);
    let color = color.unwrap_or(Color::CurrentColor);
    for &property in ShorthandId::Border.metadata().longhands {
        let value = match property.metadata().value_type {
            crate::ValueType::BorderWidth => PropertyValue::BorderWidth(width),
            crate::ValueType::BorderStyle => PropertyValue::BorderStyle(style),
            crate::ValueType::Color => PropertyValue::Color(color),
            _ => unreachable!("validated border longhand family"),
        };
        block.declarations.push(Declaration {
            property,
            value: DeclaredValue::Value(value),
            important,
        });
    }
}

fn expand_white_space(block: &mut DeclarationBlock, value: &str, important: bool) {
    let (collapse, wrap) = match value.trim().to_ascii_lowercase().as_str() {
        "normal" => ("collapse", "wrap"),
        "pre" => ("preserve", "nowrap"),
        "pre-wrap" => ("preserve", "wrap"),
        "pre-line" => ("preserve-breaks", "wrap"),
        _ => {
            block.errors.push(DeclarationError {
                name: "white-space".to_owned(),
                value: value.to_owned(),
                kind: DeclarationErrorKind::InvalidValue,
            });
            return;
        },
    };
    push_longhand(block, "white-space-collapse", collapse, important);
    push_longhand(block, "text-wrap-mode", wrap, important);
}

fn expand_css_wide_shorthand(
    block: &mut DeclarationBlock,
    shorthand: ShorthandId,
    keyword: &str,
    important: bool,
) {
    for &property in shorthand.metadata().longhands {
        block.declarations.push(Declaration {
            property,
            value: DeclaredValue::parse(property, keyword).expect("CSS-wide keyword"),
            important,
        });
    }
}

/// Parse a style-rule declaration block. Invalid declarations are retained as
/// diagnostics while valid declarations continue through CSS error recovery.
pub fn parse_declaration_block(input: &str) -> DeclarationBlock {
    let clean = without_comments(input);
    let mut block = DeclarationBlock::default();
    for raw in split_top_level(&clean, ';') {
        let declaration = raw.trim();
        if declaration.is_empty() {
            continue;
        }
        let Some(colon) = split_top_level(declaration, ':')
            .first()
            .map(|head| head.len())
        else {
            continue;
        };
        if colon == declaration.len() {
            block.errors.push(DeclarationError {
                name: declaration.to_owned(),
                value: String::new(),
                kind: DeclarationErrorKind::MalformedDeclaration,
            });
            continue;
        }
        let name = declaration[..colon].trim().to_ascii_lowercase();
        let (value, important) = strip_important(&declaration[colon + 1..]);
        if push_longhand(&mut block, &name, value, important) {
            continue;
        }
        let Some(shorthand) = ShorthandId::from_css_name(&name) else {
            block.errors.push(DeclarationError {
                name,
                value: value.to_owned(),
                kind: DeclarationErrorKind::UnknownProperty,
            });
            continue;
        };
        if matches!(
            value.to_ascii_lowercase().as_str(),
            "initial" | "inherit" | "unset"
        ) {
            expand_css_wide_shorthand(&mut block, shorthand, value, important);
        } else if expand_box_shorthand(&mut block, shorthand, value, important) {
        } else if shorthand == ShorthandId::Transition {
            expand_transition(&mut block, value, important);
        } else if shorthand == ShorthandId::Animation {
            expand_animation(&mut block, value, important);
        } else if shorthand == ShorthandId::Border {
            expand_border(&mut block, value, important);
        } else if shorthand == ShorthandId::WhiteSpace {
            expand_white_space(&mut block, value, important);
        }
    }
    block
}

/// Stylesheet origin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Origin {
    UserAgent,
    User,
    Author,
}

/// Layer position inside one origin. Layer numbers increase in declaration
/// order. Unlayered normal declarations outrank layered normal declarations;
/// important declarations reverse that order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CascadeLayer {
    Layer(u32),
    Unlayered,
}

/// Packed selector specificity. The selectors crate supplies this encoding.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Specificity(pub u32);

impl Specificity {
    pub const INLINE: Self = Self(u32::MAX);
}

/// One declaration whose selector and media condition already matched.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchedDeclaration {
    pub declaration: Declaration,
    pub origin: Origin,
    pub layer: CascadeLayer,
    pub specificity: Specificity,
    pub source_order: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Priority {
    cascade_level: u8,
    layer: u32,
    specificity: Specificity,
    source_order: u64,
}

impl Priority {
    fn new(declaration: &MatchedDeclaration) -> Self {
        let important = declaration.declaration.important;
        let cascade_level = match (important, declaration.origin) {
            (false, Origin::UserAgent) => 0,
            (false, Origin::User) => 1,
            (false, Origin::Author) => 2,
            (true, Origin::Author) => 3,
            (true, Origin::User) => 4,
            (true, Origin::UserAgent) => 5,
        };
        let layer = match (important, declaration.layer) {
            (false, CascadeLayer::Layer(order)) => order,
            (false, CascadeLayer::Unlayered) => u32::MAX,
            (true, CascadeLayer::Unlayered) => 0,
            (true, CascadeLayer::Layer(order)) => u32::MAX - order,
        };
        Self {
            cascade_level,
            layer,
            specificity: declaration.specificity,
            source_order: declaration.source_order,
        }
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.cascade_level,
            self.layer,
            self.specificity,
            self.source_order,
        )
            .cmp(&(
                other.cascade_level,
                other.layer,
                other.specificity,
                other.source_order,
            ))
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Resolve a set of already-matched declarations into one concrete style.
pub fn cascade(
    parent: Option<&ComputedValues>,
    declarations: impl IntoIterator<Item = MatchedDeclaration>,
) -> ComputedValues {
    let mut winners = (0..PropertyId::ALL.len())
        .map(|_| None)
        .collect::<Vec<Option<(Priority, DeclaredValue)>>>();
    for matched in declarations {
        let index = matched.declaration.property as usize;
        let priority = Priority::new(&matched);
        let replace = winners[index]
            .as_ref()
            .is_none_or(|(current, _)| priority >= *current);
        if replace {
            winners[index] = Some((priority, matched.declaration.value));
        }
    }

    let initial = ComputedValues::default();
    let mut result = parent.map(ComputedValues::for_child).unwrap_or_default();
    for (index, winner) in winners.into_iter().enumerate() {
        let Some((_, value)) = winner else {
            continue;
        };
        let property = PropertyId::ALL[index];
        match value {
            DeclaredValue::Value(value) => {
                result
                    .set(property, value)
                    .unwrap_or_else(|_| panic!("generated value type mismatch for {property:?}"));
            },
            DeclaredValue::Initial => result.copy_property_from(property, &initial),
            DeclaredValue::Inherit => {
                result.copy_property_from(property, parent.unwrap_or(&initial));
            },
            DeclaredValue::Unset => {
                if property.metadata().inherited {
                    result.copy_property_from(property, parent.unwrap_or(&initial));
                } else {
                    result.copy_property_from(property, &initial);
                }
            },
        }
    }
    result
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UserAgent => "user-agent",
            Self::User => "user",
            Self::Author => "author",
        })
    }
}
