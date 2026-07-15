//! Style rules joining selectors, media conditions, declarations, and cascade.

use std::{error::Error, fmt};

use crate::ComputedValues;
use crate::cascade::{
    CascadeLayer, DeclarationBlock, MatchedDeclaration, Origin, cascade, parse_declaration_block,
};
use crate::media::{Device, MediaParseError, MediaQueryList};
use crate::selector::{Element, SelectorList, SelectorParseError};

/// A recoverable stylesheet parse diagnostic. Invalid rules are dropped while
/// later rules continue parsing, matching CSS's rule-level recovery model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StylesheetDiagnostic {
    pub prelude: String,
    pub message: String,
}

/// A parsed rule sheet for the bounded Livery lane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stylesheet {
    rules: Vec<StyleRule>,
    keyframes: Vec<Keyframes>,
    diagnostics: Vec<StylesheetDiagnostic>,
}

impl Stylesheet {
    /// Parse style rules and top-level `@media` groups. Other at-rules are
    /// skipped as unsupported lane input and retained as diagnostics.
    pub fn parse(input: &str, origin: Origin) -> Self {
        Self::parse_with_offset(input, origin, 0)
    }

    /// Parse a sheet whose first rule follows `source_order` rules already
    /// loaded at the same origin.
    pub fn parse_with_offset(input: &str, origin: Origin, source_order: u64) -> Self {
        let clean = without_comments(input);
        let mut sheet = Self::default();
        parse_rule_list(&clean, origin, None, source_order, &mut sheet);
        sheet
    }

    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }

    pub fn diagnostics(&self) -> &[StylesheetDiagnostic] {
        &self.diagnostics
    }

    pub fn keyframes(&self) -> &[Keyframes] {
        &self.keyframes
    }

    pub fn into_rules(self) -> Vec<StyleRule> {
        self.rules
    }

    pub fn into_keyframes(self) -> Vec<Keyframes> {
        self.keyframes
    }
}

/// One named keyframe block. The first animation gate consumes the opacity
/// declaration from these frames; other declarations remain available for
/// later property ratchets.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframes {
    name: Box<str>,
    frames: Vec<Keyframe>,
}

impl Keyframes {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn frames(&self) -> &[Keyframe] {
        &self.frames
    }
}

/// A keyframe declaration block at a normalized offset in the animation.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    offset: f32,
    declarations: DeclarationBlock,
}

impl Keyframe {
    pub fn offset(&self) -> f32 {
        self.offset
    }

    pub fn declarations(&self) -> &DeclarationBlock {
        &self.declarations
    }
}

#[derive(Debug)]
pub enum StyleRuleError {
    Selector(SelectorParseError),
    Media(MediaParseError),
}

impl fmt::Display for StyleRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selector(error) => write!(formatter, "selector: {error}"),
            Self::Media(error) => write!(formatter, "media query: {error}"),
        }
    }
}

impl Error for StyleRuleError {}

/// One style rule in source order. Declaration parse errors remain attached to
/// the rule as diagnostics; valid declarations still participate in cascade.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    selectors: SelectorList,
    declarations: DeclarationBlock,
    media: Option<MediaQueryList>,
    origin: Origin,
    layer: CascadeLayer,
    source_order: u64,
}

impl StyleRule {
    pub fn parse(
        selectors: &str,
        declarations: &str,
        media: Option<&str>,
        origin: Origin,
        layer: CascadeLayer,
        source_order: u64,
    ) -> Result<Self, StyleRuleError> {
        Ok(Self {
            selectors: SelectorList::parse(selectors).map_err(StyleRuleError::Selector)?,
            declarations: parse_declaration_block(declarations),
            media: media
                .map(str::parse)
                .transpose()
                .map_err(StyleRuleError::Media)?,
            origin,
            layer,
            source_order,
        })
    }

    pub fn declaration_block(&self) -> &DeclarationBlock {
        &self.declarations
    }

    pub fn matched_declarations<E>(&self, element: &E, device: &Device) -> Vec<MatchedDeclaration>
    where
        E: Element<Impl = crate::selector::LiverySelectorImpl>,
    {
        if self
            .media
            .as_ref()
            .is_some_and(|condition| !condition.matches(device))
        {
            return Vec::new();
        }
        let Some(specificity) = self.selectors.matching_specificity(element) else {
            return Vec::new();
        };
        self.declarations
            .declarations
            .iter()
            .enumerate()
            .map(|(index, declaration)| MatchedDeclaration {
                declaration: declaration.clone(),
                origin: self.origin,
                layer: self.layer,
                specificity,
                source_order: self
                    .source_order
                    .saturating_mul(65_536)
                    .saturating_add(index as u64),
            })
            .collect()
    }
}

/// Match and cascade a hand-built rule corpus for one element.
pub fn cascade_rules<E>(
    parent: Option<&ComputedValues>,
    element: &E,
    device: &Device,
    rules: &[StyleRule],
) -> ComputedValues
where
    E: Element<Impl = crate::selector::LiverySelectorImpl>,
{
    cascade(
        parent,
        rules
            .iter()
            .flat_map(|rule| rule.matched_declarations(element, device)),
    )
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

fn find_open_brace(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in input[start..].char_indices() {
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
            '{' => return Some(start + offset),
            ';' => return None,
            _ => {},
        }
    }
    None
}

fn find_close_brace(input: &str, open: usize) -> Option<usize> {
    let mut depth = 1_u32;
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in input[open + 1..].char_indices() {
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
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + 1 + offset);
                }
            },
            _ => {},
        }
    }
    None
}

fn keyframes_name(prelude: &str) -> Option<&str> {
    let mut parts = prelude.split_whitespace();
    let at_rule = parts.next()?;
    if !(at_rule.eq_ignore_ascii_case("@keyframes")
        || at_rule.eq_ignore_ascii_case("@-webkit-keyframes"))
    {
        return None;
    }
    let name = parts.next()?.trim();
    (parts.next().is_none() && !name.is_empty()).then_some(name)
}

fn keyframe_offset(prelude: &str) -> Option<f32> {
    match prelude.trim().to_ascii_lowercase().as_str() {
        "from" => Some(0.0),
        "to" => Some(1.0),
        value => value
            .strip_suffix('%')
            .and_then(|value| value.trim().parse::<f32>().ok())
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            .map(|value| value / 100.0),
    }
}

fn parse_keyframes(name: &str, input: &str, sheet: &mut Stylesheet) -> Option<Keyframes> {
    let mut frames = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let Some(non_space) = input[cursor..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(offset, _)| cursor + offset)
        else {
            break;
        };
        cursor = non_space;
        let Some(open) = find_open_brace(input, cursor) else {
            let tail = input[cursor..].trim();
            if !tail.is_empty() {
                sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: tail.to_owned(),
                    message: "expected a keyframe block".to_owned(),
                });
            }
            break;
        };
        let prelude = input[cursor..open].trim();
        let Some(close) = find_close_brace(input, open) else {
            sheet.diagnostics.push(StylesheetDiagnostic {
                prelude: prelude.to_owned(),
                message: "unclosed keyframe block".to_owned(),
            });
            break;
        };
        let body = &input[open + 1..close];
        let offsets = prelude
            .split(',')
            .filter_map(keyframe_offset)
            .collect::<Vec<_>>();
        if offsets.is_empty() {
            sheet.diagnostics.push(StylesheetDiagnostic {
                prelude: prelude.to_owned(),
                message: "invalid keyframe selector".to_owned(),
            });
        } else {
            let declarations = parse_declaration_block(body);
            for offset in offsets {
                frames.push(Keyframe {
                    offset,
                    declarations: declarations.clone(),
                });
            }
        }
        cursor = close + 1;
    }
    if frames.is_empty() {
        return None;
    }
    frames.sort_by(|left, right| left.offset.total_cmp(&right.offset));
    Some(Keyframes {
        name: name.into(),
        frames,
    })
}

fn parse_rule_list(
    input: &str,
    origin: Origin,
    media: Option<&str>,
    source_order_offset: u64,
    sheet: &mut Stylesheet,
) {
    let mut cursor = 0;
    while cursor < input.len() {
        let Some(non_space) = input[cursor..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(offset, _)| cursor + offset)
        else {
            break;
        };
        cursor = non_space;

        let Some(open) = find_open_brace(input, cursor) else {
            let tail = input[cursor..].trim();
            if !tail.is_empty() {
                sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: tail.to_owned(),
                    message: "expected a rule block".to_owned(),
                });
            }
            break;
        };
        let prelude = input[cursor..open].trim();
        let Some(close) = find_close_brace(input, open) else {
            sheet.diagnostics.push(StylesheetDiagnostic {
                prelude: prelude.to_owned(),
                message: "unclosed rule block".to_owned(),
            });
            break;
        };
        let body = &input[open + 1..close];

        if let Some(name) = keyframes_name(prelude) {
            if media.is_some() {
                sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: prelude.to_owned(),
                    message: "keyframes inside media groups are outside the first lane".to_owned(),
                });
            } else if let Some(keyframes) = parse_keyframes(name, body, sheet) {
                sheet.keyframes.push(keyframes);
            }
        } else if let Some(condition) = prelude
            .get(..6)
            .filter(|prefix| prefix.eq_ignore_ascii_case("@media"))
            .and_then(|_| prelude.get(6..))
            .filter(|rest| {
                rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('(')
            })
            .map(str::trim)
        {
            if media.is_some() {
                sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: prelude.to_owned(),
                    message: "nested media groups are outside the first lane".to_owned(),
                });
            } else if condition.is_empty() {
                sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: prelude.to_owned(),
                    message: "empty media query".to_owned(),
                });
            } else {
                parse_rule_list(body, origin, Some(condition), source_order_offset, sheet);
            }
        } else if prelude.starts_with('@') {
            sheet.diagnostics.push(StylesheetDiagnostic {
                prelude: prelude.to_owned(),
                message: "unsupported at-rule".to_owned(),
            });
        } else {
            let source_order = source_order_offset.saturating_add(sheet.rules.len() as u64);
            match StyleRule::parse(
                prelude,
                body,
                media,
                origin,
                CascadeLayer::Unlayered,
                source_order,
            ) {
                Ok(rule) => sheet.rules.push(rule),
                Err(error) => sheet.diagnostics.push(StylesheetDiagnostic {
                    prelude: prelude.to_owned(),
                    message: error.to_string(),
                }),
            }
        }
        cursor = close + 1;
    }
}
