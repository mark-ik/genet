/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Portable document model — what engines produce.
//!
//! This module defines the universal semantic shape every engine renders
//! into. The model carries:
//!
//! - **Structure**: `Block` / `InlineSpan` (headings, paragraphs,
//!   lists, links, etc.) — what content there is.
//! - **Semantic intent**: `FeedHeader`, `FeedEntry`, `MetadataRow`, `Badge`
//!   — when a source format has block-level meaning beyond "this is a
//!   paragraph", the engine can name it. Intelligence layers downstream
//!   (search, summarise, recommend, recall) match on these intents, not
//!   just text.
//! - **Provenance**: where the content came from (canonical URI, source
//!   engine, fetch time, source label).
//! - **Trust state**: how authenticated the source is (Trusted / Tofu /
//!   Insecure / Broken / Unknown).
//! - **Diagnostics**: parser / render warnings the engine wants to surface
//!   (unsupported construct, degraded rendering, parse warnings).
//!
//! ## Spec faithfulness
//!
//! Protocol engines (gemini, gopher, RSS/Atom, finger) populate semantic
//! variants only when the source spec actually says them — RSS `<item>`
//! becomes `FeedEntry`, finger `Login: alice` becomes `MetadataRow`. The
//! engines do not invent semantics the spec doesn't say. File-format
//! engines (markdown, the knot note format) can be richer because the
//! format owns its own semantics.
//!
//! ## Round-trip
//!
//! [`EngineDocument::to_markdown`] and [`EngineDocument::to_gemini`] live
//! in the `render` submodule and let clips / notes export back into native
//! CommonMark / gemtext.

use serde::{Deserialize, Serialize};

mod block_provenance;
mod evaluate;
mod render;
mod transclude;
pub use evaluate::{
    BlockEvaluator, BlockEvaluators, EvalOutcome, EvalOutput, EvaluationPolicy, evaluate_blocks,
    parse_eval,
};
pub use render::GophermapContext;
pub use transclude::{
    Fetched, TranscludeOutcome, TransclusionPolicy, parse_include, resolve_transclusions,
};

pub use block_provenance::{BlockProvenance, BlockProvenanceMap, ResolvedProvenance};

/// A rendered document.
///
/// Portable, serializable, host-neutral. No layout coordinates — those come
/// later in `platen` once a real surface and font system are bound.
///
/// **A11y note:** the document maps to an AccessKit `Role::Document` node.
/// `title` is the document's accessible name; `lang` is its declared
/// language (BCP 47). Engines that don't see a language declaration leave
/// `lang` as `None` and the host fills it from environment defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineDocument {
    /// The address the engine was asked to render. May differ from
    /// `provenance.canonical_uri` when the host followed a redirect.
    pub address: String,
    pub title: Option<String>,
    pub content_type: String,
    /// BCP 47 language tag (e.g. `"en"`, `"en-GB"`, `"ja"`). `None` when the
    /// engine had no signal — host supplies a default during projection.
    #[serde(default)]
    pub lang: Option<String>,
    /// Where the document came from. Defaulted to `Default` on engines that
    /// haven't been updated yet; new engines should populate at least
    /// `source_kind` and `canonical_uri`.
    #[serde(default)]
    pub provenance: DocumentProvenance,
    /// How authenticated the source is. Default is `Unknown`; the host or
    /// the engine fills it in once enough information is available.
    #[serde(default)]
    pub trust: DocumentTrustState,
    /// Parser / render warnings worth surfacing to the user or to a debug
    /// overlay. Defaults to empty.
    #[serde(default)]
    pub diagnostics: Vec<DocumentDiagnostic>,
    pub blocks: Vec<Block>,
}

impl EngineDocument {
    /// Walk every block (and nested blocks inside quotes / list items),
    /// yielding inline spans in document order. Useful for link extraction
    /// and plain-text summarisation.
    pub fn walk_inline_spans(&self) -> Vec<&InlineSpan> {
        let mut out = Vec::new();
        for block in &self.blocks {
            collect_block_spans(block, &mut out);
        }
        out
    }

    /// Extract every link target referenced by the document, in document
    /// order. Walks both inline `Link` spans inside structural blocks and
    /// the URL fields of semantic blocks (`FeedHeader.source_url`,
    /// `FeedEntry.article_url` / `source_url`).
    pub fn outgoing_links(&self) -> Vec<&str> {
        let mut links = Vec::new();
        for block in &self.blocks {
            collect_block_link_urls(block, &mut links);
        }
        links
    }
}

/// Where the document came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentProvenance {
    /// Stable engine ID that produced this document (e.g.
    /// `"nematic.markdown"`). Lets downstream consumers know which parser
    /// shape produced the output.
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Canonical address of the document. Often equal to
    /// [`EngineDocument::address`] but can differ when a fetcher followed
    /// redirects or canonicalised a URL.
    #[serde(default)]
    pub canonical_uri: Option<String>,
    /// RFC 3339 timestamp; populated by the host's fetch layer.
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// Human-readable label for the source ("Wikipedia", "alice's blog").
    #[serde(default)]
    pub source_label: Option<String>,
}

impl DocumentProvenance {
    /// Convenience constructor for engines: records the engine's own ID and
    /// the address it was asked to render. Hosts add `fetched_at` and
    /// `source_label` later.
    pub fn for_engine(engine_id: &str, address: &str) -> Self {
        Self {
            source_kind: Some(engine_id.to_string()),
            canonical_uri: Some(address.to_string()),
            fetched_at: None,
            source_label: None,
        }
    }
}

/// How authenticated the document's source is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentTrustState {
    /// Source verified through a chain of trust (TLS root, signature,
    /// signed envelope, etc.).
    Trusted,
    /// First contact with this peer was accepted on trust ("trust on first
    /// use"). Re-fetches must verify the same key.
    Tofu,
    /// Loaded over an unauthenticated transport (plain HTTP, file://).
    Insecure,
    /// Verification was attempted and failed (cert mismatch, signature
    /// invalid, key changed). Content rendered with a warning.
    Broken,
    /// Trust state has not been evaluated.
    #[default]
    Unknown,
}

/// A note the engine attaches to the document about parse / render quality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentDiagnostic {
    /// A spec construct exists in the source but isn't supported by this
    /// engine; some content was lost.
    UnsupportedConstruct(String),
    /// Content rendered with reduced fidelity (HTML tags stripped, links
    /// flattened, etc.).
    DegradedRendering(String),
    /// Recoverable parser warning.
    ParseWarning(String),
    /// Engine fell back to raw-source presentation; consumers may want to
    /// expose the unparsed bytes.
    RawSourceFallback,
}

/// A block-level region of an [`EngineDocument`].
///
/// Each variant maps to an AccessKit role; the projection layer lifts these
/// into an a11y / automation tree without any host-specific information.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    /// AccessKit `Role::Heading` with the heading level set on the node.
    Heading { level: u8, spans: Vec<InlineSpan> },
    /// AccessKit `Role::Paragraph`.
    Paragraph { spans: Vec<InlineSpan> },
    /// AccessKit `Role::CodeSample`.
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    /// AccessKit `Role::Blockquote`.
    Quote { blocks: Vec<Block> },
    /// AccessKit `Role::List`.
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    /// AccessKit `Role::Image`.
    Image { url: String, alt: String },
    /// AccessKit `Role::Pre`.
    Preformatted { text: String },
    /// AccessKit `Role::Separator`.
    Rule,
    /// Feed-level header (RSS `<channel>`, Atom `<feed>` top-level
    /// metadata). Distinguishes "this is the feed itself" from "this is an
    /// entry in the feed" so projection layers can render them
    /// differently.
    FeedHeader {
        title: String,
        subtitle: Option<String>,
        summary: Option<String>,
        source_url: Option<String>,
    },
    /// One entry in a syndication feed (RSS `<item>` / Atom `<entry>`).
    FeedEntry {
        title: String,
        date: Option<String>,
        summary: Option<String>,
        article_url: Option<String>,
        source_url: Option<String>,
    },
    /// Label / value pair (`Login: alice`, `Language: en-US`,
    /// `Last-Modified: …`). Projection renders as a definition-list row.
    MetadataRow { label: String, value: String },
    /// Short status / annotation marker (trust state notice, "raw source"
    /// affordance). Visual hint for projection; not free-flowing text.
    Badge { text: String },
    /// A table: an optional header row plus body rows, each a list of cells, each
    /// cell inline spans, with per-column alignment. Flat — no rowspan / colspan
    /// (the carve `^` / `<` span syntax is a later rung). AccessKit `Role::Table`.
    Table {
        /// Per-column alignment; `alignments[i]` applies to column `i`. May be
        /// shorter than the widest row; missing columns default to
        /// [`TableAlignment::None`].
        alignments: Vec<TableAlignment>,
        /// Header cells, one per column, or empty when the table has no header row.
        header: Vec<Vec<InlineSpan>>,
        /// Body rows; each row is a list of cells, each cell a list of inline spans.
        rows: Vec<Vec<Vec<InlineSpan>>>,
    },
}

/// A table column's text alignment (djot / markdown `:---`, `:---:`, `---:`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableAlignment {
    /// No explicit alignment marker; the renderer's default (typically left).
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// An inline-level span inside a [`Block`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineSpan {
    Text(String),
    Code(String),
    Emphasis(Vec<InlineSpan>),
    Strong(Vec<InlineSpan>),
    Link {
        url: String,
        title: Option<String>,
        spans: Vec<InlineSpan>,
        /// Open predicate IRI (statements-over-schema): the link's `rel`, e.g. a
        /// djot `[[Topic]]{rel=schema:cites}`. `None` for a plain link. Knot
        /// ingestion maps this onto a kernel `Semantic` edge predicate.
        #[serde(default)]
        predicate: Option<String>,
    },
    LineBreak,
    SoftBreak,
}

// =============================================================================
// Inline / link traversal helpers
// =============================================================================

/// Flatten a span list into a plain-text string suitable for accessible
/// names and snippet extraction. `Code` contributes its inner text; styling
/// containers (`Emphasis`/`Strong`) and `Link` contribute their inner span
/// text recursively. `SoftBreak` becomes a space, `LineBreak` becomes a
/// newline.
pub fn inline_text(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        append_inline_text(span, &mut out);
    }
    out
}

fn append_inline_text(span: &InlineSpan, out: &mut String) {
    match span {
        InlineSpan::Text(text) | InlineSpan::Code(text) => out.push_str(text),
        InlineSpan::Emphasis(spans) | InlineSpan::Strong(spans) => {
            for inner in spans {
                append_inline_text(inner, out);
            }
        }
        InlineSpan::Link { spans, .. } => {
            for inner in spans {
                append_inline_text(inner, out);
            }
        }
        InlineSpan::SoftBreak => out.push(' '),
        InlineSpan::LineBreak => out.push('\n'),
    }
}

fn collect_block_spans<'a>(block: &'a Block, out: &mut Vec<&'a InlineSpan>) {
    match block {
        Block::Heading { spans, .. } | Block::Paragraph { spans } => {
            for span in spans {
                out.push(span);
            }
        }
        Block::Quote { blocks } => {
            for inner in blocks {
                collect_block_spans(inner, out);
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for inner in item {
                    collect_block_spans(inner, out);
                }
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header.iter().chain(rows.iter().flatten()) {
                for span in cell {
                    out.push(span);
                }
            }
        }
        Block::CodeBlock { .. }
        | Block::Image { .. }
        | Block::Preformatted { .. }
        | Block::Rule
        | Block::FeedHeader { .. }
        | Block::FeedEntry { .. }
        | Block::MetadataRow { .. }
        | Block::Badge { .. } => {}
    }
}

fn collect_block_link_urls<'a>(block: &'a Block, out: &mut Vec<&'a str>) {
    match block {
        Block::Heading { spans, .. } | Block::Paragraph { spans } => {
            for span in spans {
                collect_link_urls(span, out);
            }
        }
        Block::Quote { blocks } => {
            for inner in blocks {
                collect_block_link_urls(inner, out);
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for inner in item {
                    collect_block_link_urls(inner, out);
                }
            }
        }
        Block::FeedHeader { source_url, .. } => {
            if let Some(url) = source_url {
                out.push(url.as_str());
            }
        }
        Block::FeedEntry {
            article_url,
            source_url,
            ..
        } => {
            if let Some(url) = article_url {
                out.push(url.as_str());
            }
            if let Some(url) = source_url {
                out.push(url.as_str());
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header.iter().chain(rows.iter().flatten()) {
                for span in cell {
                    collect_link_urls(span, out);
                }
            }
        }
        Block::CodeBlock { .. }
        | Block::Image { .. }
        | Block::Preformatted { .. }
        | Block::Rule
        | Block::MetadataRow { .. }
        | Block::Badge { .. } => {}
    }
}

fn collect_link_urls<'a>(span: &'a InlineSpan, out: &mut Vec<&'a str>) {
    match span {
        InlineSpan::Link { url, spans, .. } => {
            out.push(url.as_str());
            for inner in spans {
                collect_link_urls(inner, out);
            }
        }
        InlineSpan::Emphasis(spans) | InlineSpan::Strong(spans) => {
            for inner in spans {
                collect_link_urls(inner, out);
            }
        }
        InlineSpan::Text(_)
        | InlineSpan::Code(_)
        | InlineSpan::LineBreak
        | InlineSpan::SoftBreak => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(blocks: Vec<Block>) -> EngineDocument {
        EngineDocument {
            address: "doc:1".into(),
            title: None,
            content_type: "text/plain".into(),
            lang: None,
            provenance: DocumentProvenance::default(),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        }
    }

    #[test]
    fn provenance_for_engine_records_kind_and_uri() {
        let p = DocumentProvenance::for_engine("nematic.markdown", "file:///x.md");
        assert_eq!(p.source_kind.as_deref(), Some("nematic.markdown"));
        assert_eq!(p.canonical_uri.as_deref(), Some("file:///x.md"));
        assert!(p.fetched_at.is_none());
    }

    #[test]
    fn trust_state_default_is_unknown() {
        let t: DocumentTrustState = Default::default();
        assert_eq!(t, DocumentTrustState::Unknown);
    }

    #[test]
    fn outgoing_links_walks_feed_entry_urls() {
        let document = doc(vec![
            Block::FeedHeader {
                title: "Feed".into(),
                subtitle: None,
                summary: None,
                source_url: Some("https://feed.test/".into()),
            },
            Block::FeedEntry {
                title: "Entry".into(),
                date: None,
                summary: None,
                article_url: Some("https://feed.test/post-1".into()),
                source_url: Some("https://feed.test/".into()),
            },
        ]);
        assert_eq!(
            document.outgoing_links(),
            vec![
                "https://feed.test/",
                "https://feed.test/post-1",
                "https://feed.test/",
            ]
        );
    }
}
