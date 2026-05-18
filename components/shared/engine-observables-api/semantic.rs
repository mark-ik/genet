/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Common semantic facts every lane that has a document publishes —
//! plus engine-specific extensions for richer protocol-native shape.
//!
//! Hekate's indexing / search / preview pipeline consumes
//! [`SemanticQuery`] only — uniform across lanes. Apparatus's
//! inspector pane can downcast to a lane-specific extension when
//! present (e.g., `if let Some(html) = doc.as_html_ext()`).
//!
//! Cf. Hekate doc §"Semantic Plane". Per Mark's correction: don't
//! force one fake tree model across lanes — use common-minimum +
//! engine-specific extensions instead.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use crate::types::{Lang, SourceNodeId, SourceRange};

/// Common-minimum semantic queries every lane with a document
/// publishes. The trait is generic over `NodeId` so consumers that
/// hold a concrete impl can index without boxing; engine-agnostic
/// dispatch (Hekate) reads `SourceNodeId` via the inferred
/// `as_source_id` helper each impl defines.
pub trait SemanticQuery {
    type NodeId: Copy + Eq + std::hash::Hash;

    /// Document title (HTML `<title>`, Atom `<title>`, RSS channel
    /// title, Gemini frontmatter title, Scroll frontmatter, etc.).
    fn title(&self) -> Option<&str>;

    /// Document language (HTML `lang` attribute, Atom `xml:lang`,
    /// etc.). BCP 47 tag.
    fn language(&self) -> Option<&Lang>;

    /// Heading hierarchy. Lane impls pick their own ordering — most
    /// will iterate in document order.
    fn headings<'a>(&'a self) -> Box<dyn Iterator<Item = HeadingInfo> + 'a>;

    /// Outbound links.
    fn links<'a>(&'a self) -> Box<dyn Iterator<Item = LinkInfo> + 'a>;

    /// Named anchors (`<a name>`, `#fragment`-target headings, etc.).
    fn anchors<'a>(&'a self) -> Box<dyn Iterator<Item = AnchorInfo> + 'a>;

    /// Nodes matching a generic semantic role (e.g., `Main`,
    /// `Navigation`). Useful for reader-mode extraction.
    fn nodes_by_role<'a>(
        &'a self,
        role: SemanticRole,
    ) -> Box<dyn Iterator<Item = Self::NodeId> + 'a>;

    /// Text content of a node, if it has any (text leaf, headings,
    /// link text).
    fn text_range(&self, node: Self::NodeId) -> Option<&str>;

    /// Source-byte range a node corresponds to. Lanes that don't
    /// track source spans return `None`.
    fn source_range(&self, node: Self::NodeId) -> Option<SourceRange>;
}

/// One heading and its level.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct HeadingInfo {
    pub source_node: SourceNodeId,
    /// 1..=6 for HTML, lane-specific for others (Markdown allows
    /// 1..=6 too; Scroll's headings flatten; Gemini has one level).
    pub level: u8,
    pub text: String,
}

/// One outbound link.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct LinkInfo {
    pub source_node: SourceNodeId,
    pub href: String,
    /// Visible link text (may be empty for image links).
    pub text: String,
}

/// One named anchor (target of `#fragment` navigation).
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct AnchorInfo {
    pub source_node: SourceNodeId,
    pub name: String,
}

/// Generic semantic role consumers can ask about, independent of any
/// specific markup language. Map onto each lane's native vocabulary.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum SemanticRole {
    Main,
    Navigation,
    Article,
    Section,
    Aside,
    Header,
    Footer,
    Heading,
    Paragraph,
    List,
    ListItem,
    Quote,
    Code,
    Form,
    #[default]
    Generic,
}
