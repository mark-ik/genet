/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Djot substrate for knot bodies (design doc §10).
//!
//! Demonstrates that djot's *native* constructs replace the CommonMark knot's
//! fenced-code-as-data hacks (§2.2):
//!
//! - a **definition list** (`Term` / `: value`) becomes a [`Block::MetadataRow`]
//!   — no `metadata-row` fence;
//! - a **div with a class** (`::: feed-entry`, `::: badge`) becomes the matching
//!   semantic block, reading typed attributes via `Attributes::get_value` — no
//!   `feed-entry` code-fence-as-data;
//! - an **inline link** becomes an [`InlineSpan::Link`]; a djot `rel` attribute
//!   (`[Topic](mere://node/topic){rel="schema:cites"}`) is captured into the
//!   link's `predicate` — the statements-over-schema seam that knot→graph
//!   ingest (§10.5 Phase 4) turns into a kernel `Semantic` edge;
//! - headings / paragraphs / code blocks map structurally.
//!
//! Frontmatter is shared with the CommonMark [`super::KnotEngine`] via
//! `super::apply_frontmatter`, so a djot knot and a CommonMark knot carry
//! identical note semantics. The [`blocks_to_djot`] serializer round-trips the
//! recognized block vocabulary. Parser: `jotdown` (the Rust djot pull-parser).

use inker::{
    Block, DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput, InlineSpan, inline_text,
};
use jotdown::{Container, Event, Parser};

#[cfg(test)]
mod tests;

/// Parse a djot knot body into [`Block`]s (blocks only). See
/// [`parse_djot_knot_body_validated`] for the schema diagnostics.
pub fn parse_djot_knot_body(body: &str) -> Vec<Block> {
    parse_djot_knot_body_validated(body).0
}

/// Parse a djot knot body, returning blocks plus schema diagnostics (an unknown
/// div class, or a recognized div missing a required attribute). The recognized
/// vocabulary is declared data ([`KNOT_DIV_SCHEMA`]) — the Markdoc lesson
/// (§10.4): validate against a schema, render the unrecognized generically.
pub fn parse_djot_knot_body_validated(body: &str) -> (Vec<Block>, Vec<DocumentDiagnostic>) {
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    let mut inline = Inline::default();
    let mut heading_level: Option<u8> = None;
    let mut code_language: Option<String> = None;
    let mut div: Option<DivCtx> = None;
    let mut dl: Option<DlCtx> = None;
    // A standalone attribute line (`{title=… url=…}`) is emitted by jotdown as
    // `Event::Attributes` and applies to the element that follows. Stash it and
    // fold it into the next div (block attrs) or link (`rel`), so attachment
    // works whether jotdown folds attrs into the `Start` event or emits them
    // separately.
    let mut pending = PendingAttrs::default();

    for event in Parser::new(body) {
        match event {
            Event::Attributes(attrs) => {
                pending.title = attrs.get_value("title").map(|v| v.to_string());
                pending.url = attrs.get_value("url").map(|v| v.to_string());
                pending.date = attrs.get_value("date").map(|v| v.to_string());
                pending.rel = attrs.get_value("rel").map(|v| v.to_string());
            },
            Event::Start(Container::Heading { level, .. }, _) => {
                heading_level = Some(level as u8);
                inline.clear();
            },
            Event::End(Container::Heading { .. }) => {
                if let Some(level) = heading_level.take() {
                    out.push(Block::Heading {
                        level,
                        spans: inline.take_spans(),
                    });
                }
            },
            Event::Start(Container::Div { class }, attrs) => {
                div = Some(DivCtx {
                    class: class.to_string(),
                    title: attrs
                        .get_value("title")
                        .map(|v| v.to_string())
                        .or(pending.title.take()),
                    url: attrs
                        .get_value("url")
                        .map(|v| v.to_string())
                        .or(pending.url.take()),
                    date: attrs
                        .get_value("date")
                        .map(|v| v.to_string())
                        .or(pending.date.take()),
                });
                inline.clear();
            },
            Event::End(Container::Div { .. }) => {
                if let Some(ctx) = div.take() {
                    if let Some(diagnostic) = validate_div(&ctx) {
                        diagnostics.push(diagnostic);
                    }
                    out.push(ctx.into_block(inline.take_text()));
                }
            },
            Event::Start(Container::DescriptionList, _) => dl = Some(DlCtx::default()),
            Event::End(Container::DescriptionList) => dl = None,
            Event::Start(Container::DescriptionTerm, _) => inline.clear(),
            Event::End(Container::DescriptionTerm) => {
                if let Some(d) = dl.as_mut() {
                    d.term = inline.take_text().trim().to_string();
                }
            },
            Event::Start(Container::DescriptionDetails, _) => inline.clear(),
            Event::End(Container::DescriptionDetails) => {
                if let Some(d) = dl.as_mut() {
                    out.push(Block::MetadataRow {
                        label: d.term.clone(),
                        value: inline.take_text().trim().to_string(),
                    });
                }
            },
            Event::Start(Container::CodeBlock { language }, _) => {
                code_language = Some(language.to_string());
                inline.clear();
            },
            Event::End(Container::CodeBlock { .. }) => {
                out.push(Block::CodeBlock {
                    language: code_language.take().filter(|s| !s.is_empty()),
                    text: inline.take_text(),
                });
            },
            // Inline link: capture the destination + the `rel` attribute (the
            // statements-over-schema predicate). `rel` arrives folded into the
            // link's `Start` attrs, with a standalone-`Attributes` fallback.
            Event::Start(Container::Link(dst, _), attrs) => {
                let title = attrs.get_value("title").map(|v| v.to_string());
                let predicate = attrs
                    .get_value("rel")
                    .map(|v| v.to_string())
                    .or(pending.rel.take());
                inline.start_link(dst.to_string(), title, predicate);
            },
            Event::End(Container::Link(..)) => inline.end_link(),
            Event::End(Container::Paragraph) => {
                // A *top-level* paragraph emits a block; a paragraph inside a div
                // or description-details lets its spans accumulate for that
                // container's own `End` handler instead.
                if div.is_none() && dl.is_none() && heading_level.is_none() {
                    let spans = inline.take_spans();
                    if !inline_text(&spans).trim().is_empty() {
                        out.push(Block::Paragraph { spans });
                    }
                }
            },
            Event::Str(s) => inline.push_str(s.as_ref()),
            Event::Softbreak => inline.push_char(' '),
            Event::Hardbreak => inline.push_char('\n'),
            _ => {},
        }
    }
    (out, diagnostics)
}

fn text_spans(text: String) -> Vec<InlineSpan> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![InlineSpan::Text(text)]
    }
}

/// Accumulates inline events into [`InlineSpan`]s. Most knot inline content is
/// plain text (one `Text` span), but a djot link becomes an [`InlineSpan::Link`]
/// carrying its destination and `rel` predicate. Raw-text contexts (div bodies,
/// definition values, code) recover a flat string via [`Inline::take_text`].
///
/// PoC scope: emphasis / strong still flatten to text; links don't nest.
#[derive(Default)]
struct Inline {
    spans: Vec<InlineSpan>,
    buf: String,
    link: Option<LinkBuilder>,
}

struct LinkBuilder {
    url: String,
    title: Option<String>,
    predicate: Option<String>,
    spans: Vec<InlineSpan>,
}

impl Inline {
    fn clear(&mut self) {
        self.spans.clear();
        self.buf.clear();
        self.link = None;
    }

    fn push_str(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    fn push_char(&mut self, c: char) {
        self.buf.push(c);
    }

    /// Flush the pending text run into the active span list — the link body when
    /// inside a link, else the top level. Routing by *current* link state is why
    /// every link boundary calls this.
    fn flush_text(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = InlineSpan::Text(std::mem::take(&mut self.buf));
        match &mut self.link {
            Some(l) => l.spans.push(text),
            None => self.spans.push(text),
        }
    }

    fn start_link(&mut self, url: String, title: Option<String>, predicate: Option<String>) {
        // Links don't nest in PoC scope; close an open one defensively.
        if self.link.is_some() {
            self.end_link();
        }
        self.flush_text();
        self.link = Some(LinkBuilder {
            url,
            title,
            predicate,
            spans: Vec::new(),
        });
    }

    fn end_link(&mut self) {
        self.flush_text();
        if let Some(l) = self.link.take() {
            self.spans.push(InlineSpan::Link {
                url: l.url,
                title: l.title,
                spans: l.spans,
                predicate: l.predicate,
            });
        }
    }

    /// Finish the current run and yield the accumulated spans, leaving the
    /// accumulator empty.
    fn take_spans(&mut self) -> Vec<InlineSpan> {
        if self.link.is_some() {
            self.end_link();
        }
        self.flush_text();
        std::mem::take(&mut self.spans)
    }

    /// Like [`take_spans`](Self::take_spans) but flattened to a plain string, for
    /// raw-text contexts (div bodies, definition values, code blocks).
    fn take_text(&mut self) -> String {
        inline_text(&self.take_spans())
    }
}

#[derive(Default)]
struct PendingAttrs {
    title: Option<String>,
    url: Option<String>,
    date: Option<String>,
    rel: Option<String>,
}

struct DivCtx {
    class: String,
    title: Option<String>,
    url: Option<String>,
    date: Option<String>,
}

impl DivCtx {
    fn into_block(self, content: String) -> Block {
        let summary = {
            let trimmed = content.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        match self.class.as_str() {
            "feed-entry" => Block::FeedEntry {
                title: self.title.unwrap_or_default(),
                date: self.date,
                summary,
                article_url: self.url,
                source_url: None,
            },
            "feed-header" => Block::FeedHeader {
                title: self.title.unwrap_or_default(),
                subtitle: None,
                summary,
                source_url: self.url,
            },
            "badge" => Block::Badge {
                text: content.trim().to_string(),
            },
            // Unknown div classes degrade to a stored quote rather than being
            // dropped — the open-tail discipline (recognized core, stored tail).
            _ => Block::Quote {
                blocks: vec![Block::Paragraph {
                    spans: text_spans(content),
                }],
            },
        }
    }
}

#[derive(Default)]
struct DlCtx {
    term: String,
}

/// A recognized knot div class and the attributes it requires. The recognized
/// vocabulary is declared data (the Markdoc lesson, §10.4): the parser validates
/// divs against this table and renders unrecognized ones generically.
struct DivClassSpec {
    class: &'static str,
    required: &'static [&'static str],
}

const KNOT_DIV_SCHEMA: &[DivClassSpec] = &[
    DivClassSpec {
        class: "feed-entry",
        required: &["title"],
    },
    DivClassSpec {
        class: "feed-header",
        required: &["title"],
    },
    DivClassSpec {
        class: "badge",
        required: &[],
    },
];

fn div_spec(class: &str) -> Option<&'static DivClassSpec> {
    KNOT_DIV_SCHEMA.iter().find(|spec| spec.class == class)
}

/// Validate a div against [`KNOT_DIV_SCHEMA`]: an unknown class (rendered
/// generically) or a recognized class missing a required attribute yields one
/// diagnostic; a well-formed div yields `None`. Required-attribute checking is
/// limited to the attributes the parser captures (`title` / `url` / `date`);
/// preserving *unknown* attributes (the rest of the open tail) needs attribute
/// iteration and is a later phase.
fn validate_div(ctx: &DivCtx) -> Option<DocumentDiagnostic> {
    let Some(spec) = div_spec(&ctx.class) else {
        return Some(DocumentDiagnostic::UnsupportedConstruct(format!(
            "unknown knot div class '{}'; rendered generically",
            ctx.class
        )));
    };
    let present = |attr: &str| match attr {
        "title" => ctx.title.is_some(),
        "url" => ctx.url.is_some(),
        "date" => ctx.date.is_some(),
        _ => false,
    };
    spec.required
        .iter()
        .find(|&&attr| !present(attr))
        .map(|&attr| {
            DocumentDiagnostic::ParseWarning(format!(
                "knot '{}' div missing required attribute '{}'",
                ctx.class, attr
            ))
        })
}

/// Serialize document blocks back into a djot knot body — the dual of
/// [`parse_djot_knot_body`] (design doc §10.5 Phase 2). Semantic blocks emit as
/// the native djot constructs they parsed from: `MetadataRow` → a definition
/// list, `FeedEntry` / `FeedHeader` / `Badge` → an attributed div. Round-trips
/// on the recognized subset (see `round_trip_preserves_semantic_blocks`);
/// byte-faithful protocol-fence preservation is a later phase.
pub fn blocks_to_djot(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        emit_block(block, &mut out);
    }
    out
}

/// Write a djot pipe table: header row, a `---` separator, then body rows. Cells
/// flatten to inline text (the round-trip writer; the parser side that produces
/// `Table` lands with the live tile).
fn emit_djot_table(header: &[Vec<InlineSpan>], rows: &[Vec<Vec<InlineSpan>>], out: &mut String) {
    let cols = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let emit_row = |cells: &[Vec<InlineSpan>], out: &mut String| {
        out.push('|');
        for i in 0..cols {
            out.push(' ');
            out.push_str(&cells.get(i).map(|c| inline_text(c)).unwrap_or_default());
            out.push_str(" |");
        }
        out.push('\n');
    };
    emit_row(header, out);
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    out.push('\n');
    for r in rows {
        emit_row(r, out);
    }
}

fn emit_block(block: &Block, out: &mut String) {
    match block {
        Block::Heading { level, spans } => {
            for _ in 0..(*level).max(1) {
                out.push('#');
            }
            out.push(' ');
            out.push_str(&inline_text(spans));
            out.push('\n');
        },
        Block::Paragraph { spans } => {
            out.push_str(&inline_text(spans));
            out.push('\n');
        },
        Block::Table { header, rows, .. } => {
            emit_djot_table(header, rows, out);
        },
        Block::MetadataRow { label, value } => {
            out.push_str(": ");
            out.push_str(label);
            out.push_str("\n\n  ");
            out.push_str(value);
            out.push('\n');
        },
        Block::Badge { text } => {
            out.push_str("::: badge\n");
            out.push_str(text);
            out.push_str("\n:::\n");
        },
        Block::FeedEntry {
            title,
            date,
            summary,
            article_url,
            ..
        } => {
            emit_div_attrs(out, title, article_url.as_deref(), date.as_deref());
            out.push_str("::: feed-entry\n");
            if let Some(s) = summary {
                out.push_str(s);
                out.push('\n');
            }
            out.push_str(":::\n");
        },
        Block::FeedHeader {
            title,
            summary,
            source_url,
            ..
        } => {
            emit_div_attrs(out, title, source_url.as_deref(), None);
            out.push_str("::: feed-header\n");
            if let Some(s) = summary {
                out.push_str(s);
                out.push('\n');
            }
            out.push_str(":::\n");
        },
        Block::CodeBlock { language, text } => {
            out.push_str("```");
            if let Some(lang) = language {
                out.push_str(lang);
            }
            out.push('\n');
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        },
        Block::Preformatted { text } => {
            out.push_str("```\n");
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        },
        Block::Quote { blocks } => {
            let mut inner = String::new();
            for (i, b) in blocks.iter().enumerate() {
                if i > 0 {
                    inner.push('\n');
                }
                emit_block(b, &mut inner);
            }
            for line in inner.lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
        },
        Block::List { ordered, items } => {
            for item in items {
                let mut inner = String::new();
                for (i, b) in item.iter().enumerate() {
                    if i > 0 {
                        inner.push(' ');
                    }
                    emit_block(b, &mut inner);
                }
                out.push_str(if *ordered { "1. " } else { "- " });
                out.push_str(inner.trim());
                out.push('\n');
            }
        },
        Block::Image { url, alt } => {
            out.push_str("![");
            out.push_str(alt);
            out.push_str("](");
            out.push_str(url);
            out.push_str(")\n");
        },
        Block::Rule => out.push_str("----\n"),
    }
}

fn emit_div_attrs(out: &mut String, title: &str, url: Option<&str>, date: Option<&str>) {
    out.push_str("{title=\"");
    out.push_str(title);
    out.push('"');
    if let Some(url) = url {
        out.push_str(" url=\"");
        out.push_str(url);
        out.push('"');
    }
    if let Some(date) = date {
        out.push_str(" date=\"");
        out.push_str(date);
        out.push('"');
    }
    out.push_str("}\n");
}

/// Stable engine identifier for the experimental djot knot engine.
pub const ENGINE_ID: &str = "nematic.knot-djot";

/// Experimental knot engine whose body grammar is **djot** rather than
/// CommonMark (design doc §10). Frontmatter handling (title / provenance /
/// trust / `note_kind` / `tags`) is shared with the shipped [`super::KnotEngine`]
/// via `super::apply_frontmatter`, so a djot knot and a CommonMark knot carry
/// identical note semantics. Not registered in `engines()` (it shares the
/// `text/x-knot` content-type), so a host routes to it by [`ENGINE_ID`].
pub struct DjotKnotEngine;

impl DjotKnotEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DjotKnotEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for DjotKnotEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let (frontmatter, body) = super::split_frontmatter(&input.body);
        let (mut blocks, diagnostics) = parse_djot_knot_body_validated(body);
        // Parity with the CommonMark knot: expand protocol-tagged code fences
        // (gemtext / gopher / nex / feed-entry / …) into real blocks, then
        // rewrite `[[wikilinks]]` and `#hashtags`. Both passes are engine-agnostic
        // (they walk `Block`s), so the djot body gains the same semantics
        // and CommonMark-style fenced knots still work under the djot default.
        super::expand::expand_fenced_blocks(&mut blocks);
        super::expand::rewrite_inline_extensions(&mut blocks);
        let mut doc = EngineDocument {
            address: input.address.clone(),
            title: None,
            content_type: "text/x-knot".to_string(),
            lang: None,
            provenance: DocumentProvenance::default(),
            trust: DocumentTrustState::default(),
            diagnostics,
            blocks,
        };
        super::apply_frontmatter(
            &mut doc,
            &frontmatter,
            ENGINE_ID,
            &input.address,
            input.content_type.as_deref(),
        );
        Ok(doc)
    }
}
