//! Selector parsing and matching on the standalone `selectors` substrate.

use std::{fmt, hash::Hash};

use cssparser::{CowRcStr, Parser as CssParser, ParserInput, ToCss, match_ignore_ascii_case};
use precomputed_hash::PrecomputedHash;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{MatchingContext, matches_selector};
use selectors::parser::{
    NonTSPseudoClass, ParseRelative, Parser, PseudoElement, SelectorImpl,
    SelectorList as SubstrateSelectorList, SelectorParseErrorKind,
};

pub use selectors::Element;
pub use selectors::OpaqueElement;
pub use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
pub use selectors::bloom::BloomFilter;
pub use selectors::matching::ElementSelectorFlags;

use crate::cascade::Specificity;

fn stable_hash(value: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in value.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

/// Owned selector atom with a stable precomputed hash.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Atom {
    text: Box<str>,
    hash: u32,
}

impl Atom {
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl Default for Atom {
    fn default() -> Self {
        Self::from("")
    }
}

impl From<&str> for Atom {
    fn from(value: &str) -> Self {
        Self {
            text: value.into(),
            hash: stable_hash(value),
        }
    }
}

impl AsRef<str> for Atom {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PrecomputedHash for Atom {
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

impl ToCss for Atom {
    fn to_css<W>(&self, destination: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        cssparser::serialize_identifier(self.as_str(), destination)
    }
}

/// Attribute selector value. It serializes as a CSS string, unlike identifier
/// atoms.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttributeValue(Box<str>);

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for AttributeValue {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl ToCss for AttributeValue {
    fn to_css<W>(&self, destination: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        use fmt::Write;
        destination.write_char('"')?;
        write!(cssparser::CssStringWriter::new(destination), "{}", &self.0)?;
        destination.write_char('"')
    }
}

/// Dynamic element states used by the first Cambium selector lane.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StatePseudoClass {
    Hover,
    Active,
    Focus,
    FocusWithin,
    Disabled,
    Checked,
}

impl ToCss for StatePseudoClass {
    fn to_css<W>(&self, destination: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        destination.write_str(match self {
            Self::Hover => ":hover",
            Self::Active => ":active",
            Self::Focus => ":focus",
            Self::FocusWithin => ":focus-within",
            Self::Disabled => ":disabled",
            Self::Checked => ":checked",
        })
    }
}

impl NonTSPseudoClass for StatePseudoClass {
    type Impl = LiverySelectorImpl;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Active | Self::Hover)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            Self::Active | Self::Hover | Self::Focus | Self::FocusWithin
        )
    }
}

/// Livery does not style pseudo-elements in its first lane.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NoPseudoElement {}

impl ToCss for NoPseudoElement {
    fn to_css<W>(&self, _destination: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        match *self {}
    }
}

impl PseudoElement for NoPseudoElement {
    type Impl = LiverySelectorImpl;
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiverySelectorImpl;

impl SelectorImpl for LiverySelectorImpl {
    type ExtraMatchingData<'a> = std::marker::PhantomData<&'a ()>;
    type AttrValue = AttributeValue;
    type Identifier = Atom;
    type LocalName = Atom;
    type NamespaceUrl = Atom;
    type NamespacePrefix = Atom;
    type BorrowedLocalName = Atom;
    type BorrowedNamespaceUrl = Atom;
    type NonTSPseudoClass = StatePseudoClass;
    type PseudoElement = NoPseudoElement;
}

#[derive(Default)]
struct LiverySelectorParser;

impl<'i> Parser<'i> for LiverySelectorParser {
    type Impl = LiverySelectorImpl;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_nth_child_of(&self) -> bool {
        true
    }

    fn parse_non_ts_pseudo_class(
        &self,
        location: cssparser::SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<StatePseudoClass, cssparser::ParseError<'i, Self::Error>> {
        match_ignore_ascii_case! { &name,
            "hover" => Ok(StatePseudoClass::Hover),
            "active" => Ok(StatePseudoClass::Active),
            "focus" => Ok(StatePseudoClass::Focus),
            "focus-within" => Ok(StatePseudoClass::FocusWithin),
            "disabled" => Ok(StatePseudoClass::Disabled),
            "checked" => Ok(StatePseudoClass::Checked),
            _ => Err(location.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name)
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectorParseError(String);

impl fmt::Display for SelectorParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SelectorParseError {}

/// A parsed selector list. Matching returns the strongest specificity among
/// the selectors in the list that matched the element.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectorList(SubstrateSelectorList<LiverySelectorImpl>);

impl SelectorList {
    pub fn parse(input: &str) -> Result<Self, SelectorParseError> {
        let mut input_buffer = ParserInput::new(input);
        let mut input = CssParser::new(&mut input_buffer);
        SubstrateSelectorList::parse(&LiverySelectorParser, &mut input, ParseRelative::No)
            .map(Self)
            .map_err(|error| SelectorParseError(format!("{error:?}")))
    }

    pub fn matching_specificity<E>(&self, element: &E) -> Option<Specificity>
    where
        E: Element<Impl = LiverySelectorImpl>,
    {
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            QuirksMode::NoQuirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        self.0
            .slice()
            .iter()
            .filter(|selector| matches_selector(selector, 0, None, element, &mut context))
            .map(|selector| Specificity(selector.specificity()))
            .max()
    }
}
