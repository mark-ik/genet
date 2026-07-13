//! Style rules joining selectors, media conditions, declarations, and cascade.

use std::{error::Error, fmt};

use crate::ComputedValues;
use crate::cascade::{
    CascadeLayer, DeclarationBlock, MatchedDeclaration, Origin, cascade, parse_declaration_block,
};
use crate::media::{Device, MediaParseError, MediaQueryList};
use crate::selector::{Element, SelectorList, SelectorParseError};

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
