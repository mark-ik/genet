/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Round-trip rendering of [`EngineDocument`] back into native CommonMark
//! and gemtext.
//!
//! Lets clips and notes export back into the formats their hosts expect.
//! Spec-faithful: gemtext output flattens inline styling (gemtext has none)
//! and surfaces inline links as separate `=> url label` lines after the
//! paragraph, matching gemtext's link-line model.

use super::{Block, DocumentTrustState, EngineDocument, InlineSpan, TableAlignment, inline_text};

impl EngineDocument {
    /// Render the document as CommonMark.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            block.write_markdown(&mut out, 0);
        }
        out
    }

    /// Render the document as gemtext (`text/gemini`).
    ///
    /// Spec-faithful: emphasis / strong / inline code styling is flattened
    /// to plain text since gemtext has no inline styling. Inline `Link`
    /// spans inside paragraphs are emitted as separate `=> url label` lines
    /// after the paragraph, matching gemtext's link-line model.
    pub fn to_gemini(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            block.write_gemini(&mut out);
        }
        out
    }

    /// Render the document as a knot file (frontmatter + body).
    ///
    /// Frontmatter is written when the document has any of: a title,
    /// `provenance.canonical_uri` / `fetched_at` / `source_label`, or a
    /// non-`Unknown` trust state. The body is the same shape as
    /// `to_markdown()` for structural blocks, with semantic variants
    /// (`FeedHeader`, `FeedEntry`, `MetadataRow`, `Badge`) rendered as
    /// fenced code blocks with their protocol language tag — so they
    /// round-trip through `nematic::KnotEngine`'s polyglot fence
    /// expansion without losing semantic intent.
    ///
    /// Round-trip: `parse → to_knot → parse → equivalent document` for
    /// the document-level metadata + every structural and semantic block.
    /// `note_kind` and `tags` MetadataRow blocks are NOT re-extracted
    /// to frontmatter on re-render — they stay as MetadataRow blocks in
    /// the body, preserving their content but shifting their storage
    /// location. (Asymmetry by design; the reverse-extraction would be
    /// fragile against user-authored MetadataRows that happen to use
    /// `kind` / `tags` labels.)
    pub fn to_knot(&self) -> String {
        let mut out = String::new();
        if self.has_frontmatter_content() {
            self.write_knot_frontmatter(&mut out);
        }
        self.write_knot_body(&mut out);
        out
    }

    /// Append the knot body (blocks only — no frontmatter) to `out`.
    ///
    /// Companion to [`Self::to_knot`] for callers that emit their own
    /// frontmatter and just want the rendered block stream. Used by
    /// `nematic::knot::build_clip_knot` so the clip-aware frontmatter
    /// (which carries `note_kind` and other knot-format extensions
    /// outside [`EngineDocument`]'s shape) isn't doubled by the
    /// document-level frontmatter `to_knot` would otherwise emit.
    pub fn write_knot_body(&self, out: &mut String) {
        for block in &self.blocks {
            block.write_knot(out);
        }
    }

    /// True when [`Self::to_knot`] should emit a frontmatter block.
    fn has_frontmatter_content(&self) -> bool {
        self.title.is_some()
            || self.provenance.canonical_uri.is_some()
            || self.provenance.fetched_at.is_some()
            || self.provenance.source_label.is_some()
            || !matches!(self.trust, DocumentTrustState::Unknown)
    }

    fn write_knot_frontmatter(&self, out: &mut String) {
        out.push_str("---\n");
        if let Some(title) = &self.title {
            out.push_str(&format!("title: {title}\n"));
        }
        if let Some(source) = &self.provenance.canonical_uri {
            out.push_str(&format!("source: {source}\n"));
        }
        if let Some(captured) = &self.provenance.fetched_at {
            out.push_str(&format!("captured: {captured}\n"));
        }
        if let Some(label) = &self.provenance.source_label {
            out.push_str(&format!("source_label: {label}\n"));
        }
        let trust_str = match self.trust {
            DocumentTrustState::Trusted => Some("trusted"),
            DocumentTrustState::Tofu => Some("tofu"),
            DocumentTrustState::Insecure => Some("insecure"),
            DocumentTrustState::Broken => Some("broken"),
            DocumentTrustState::Unknown => None,
        };
        if let Some(s) = trust_str {
            out.push_str(&format!("trust: {s}\n"));
        }
        out.push_str("---\n\n");
    }
}

/// Each table row (header first, if any) as a plain-text line, cells joined by
/// " | ". For exporters whose target format has no table model (gemtext,
/// gophermap, plain text); callers wrap or prefix the lines as fits.
pub(super) fn table_lines(
    header: &[Vec<InlineSpan>],
    rows: &[Vec<Vec<InlineSpan>>],
) -> Vec<String> {
    let row_line = |cells: &[Vec<InlineSpan>]| {
        cells
            .iter()
            .map(|c| inline_text(c))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let mut lines = Vec::new();
    if !header.is_empty() {
        lines.push(row_line(header));
    }
    lines.extend(rows.iter().map(|r| row_line(r)));
    lines
}

/// A GitHub-style pipe table: header row, an alignment separator, then body rows.
/// An empty header still emits a (blank) header row so the result stays valid
/// markdown; cells render their inline markdown.
fn write_markdown_table(
    alignments: &[TableAlignment],
    header: &[Vec<InlineSpan>],
    rows: &[Vec<Vec<InlineSpan>>],
    out: &mut String,
    pad: &str,
) {
    let cols = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let write_row = |cells: &[Vec<InlineSpan>], out: &mut String| {
        out.push_str(pad);
        out.push('|');
        for i in 0..cols {
            out.push(' ');
            if let Some(cell) = cells.get(i) {
                write_inline_markdown(cell, out);
            }
            out.push_str(" |");
        }
        out.push('\n');
    };
    write_row(header, out);
    out.push_str(pad);
    out.push('|');
    for i in 0..cols {
        out.push_str(match alignments.get(i).copied().unwrap_or_default() {
            TableAlignment::None => " --- |",
            TableAlignment::Left => " :--- |",
            TableAlignment::Center => " :---: |",
            TableAlignment::Right => " ---: |",
        });
    }
    out.push('\n');
    for row in rows {
        write_row(row, out);
    }
    out.push('\n');
}

impl Block {
    fn write_markdown(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        match self {
            Self::Heading { level, spans } => {
                let level = (*level).clamp(1, 6) as usize;
                out.push_str(&pad);
                out.push_str(&"#".repeat(level));
                out.push(' ');
                write_inline_markdown(spans, out);
                out.push_str("\n\n");
            },
            Self::Paragraph { spans } => {
                out.push_str(&pad);
                write_inline_markdown(spans, out);
                out.push_str("\n\n");
            },
            Self::CodeBlock { language, text } => {
                out.push_str(&pad);
                out.push_str("```");
                if let Some(lang) = language {
                    out.push_str(lang);
                }
                out.push('\n');
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push_str("```\n\n");
            },
            Self::Quote { blocks } => {
                let mut inner = String::new();
                for block in blocks {
                    block.write_markdown(&mut inner, 0);
                }
                for line in inner.lines() {
                    out.push_str(&pad);
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            },
            Self::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&pad);
                    if *ordered {
                        out.push_str(&format!("{}. ", i + 1));
                    } else {
                        out.push_str("- ");
                    }
                    let mut first = true;
                    for block in item {
                        if first {
                            // Render the first block inline with the marker
                            // by trimming the leading newline-pair it produces.
                            let mut piece = String::new();
                            block.write_markdown(&mut piece, 0);
                            out.push_str(piece.trim_end());
                            out.push('\n');
                            first = false;
                        } else {
                            block.write_markdown(out, indent + 1);
                        }
                    }
                }
                out.push('\n');
            },
            Self::Image { url, alt } => {
                out.push_str(&pad);
                out.push_str(&format!("![{alt}]({url})\n\n"));
            },
            Self::Preformatted { text } => {
                out.push_str(&pad);
                out.push_str("```\n");
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push_str("```\n\n");
            },
            Self::Rule => out.push_str("---\n\n"),
            Self::FeedHeader {
                title,
                subtitle,
                summary,
                source_url,
            } => {
                out.push_str(&format!("# {title}\n\n"));
                if let Some(subtitle) = subtitle {
                    out.push_str(&format!("## {subtitle}\n\n"));
                }
                if let Some(summary) = summary {
                    out.push_str(summary);
                    out.push_str("\n\n");
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("[Open source]({url})\n\n"));
                }
            },
            Self::FeedEntry {
                title,
                date,
                summary,
                article_url,
                source_url,
            } => {
                out.push_str(&format!("## {title}\n\n"));
                if let Some(date) = date {
                    out.push_str(&format!("*{date}*\n\n"));
                }
                if let Some(summary) = summary {
                    out.push_str(summary);
                    out.push_str("\n\n");
                }
                if let Some(url) = article_url {
                    out.push_str(&format!("[Open article]({url})\n\n"));
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("[Open source]({url})\n\n"));
                }
            },
            Self::MetadataRow { label, value } => {
                out.push_str(&format!("**{label}:** {value}\n\n"));
            },
            Self::Badge { text } => {
                out.push_str(&format!("> *{text}*\n\n"));
            },
            Self::Table {
                alignments,
                header,
                rows,
            } => {
                write_markdown_table(alignments, header, rows, out, &pad);
            },
        }
    }

    fn write_gemini(&self, out: &mut String) {
        match self {
            Self::Table { header, rows, .. } => {
                out.push_str("```\n");
                for line in table_lines(header, rows) {
                    out.push_str(&line);
                    out.push('\n');
                }
                out.push_str("```\n");
            },
            Self::Heading { level, spans } => {
                let prefix = match level {
                    1 => "# ",
                    2 => "## ",
                    _ => "### ",
                };
                out.push_str(prefix);
                out.push_str(&inline_text(spans));
                out.push('\n');
            },
            Self::Paragraph { spans } => {
                // A link-only paragraph (e.g. a parsed `=>` line, or a bare
                // link on its own line) IS the link line — emitting its text
                // first would double it.
                let text = inline_text(spans);
                if !text.is_empty() && !is_link_only(spans) {
                    out.push_str(&text);
                    out.push('\n');
                }
                // Surface inline links as separate gemtext link lines after
                // the paragraph text.
                let mut links = Vec::new();
                for span in spans {
                    collect_link_targets(span, &mut links);
                }
                for (url, label) in links {
                    out.push_str("=> ");
                    out.push_str(&url);
                    if !label.is_empty() && label != url {
                        out.push(' ');
                        out.push_str(&label);
                    }
                    out.push('\n');
                }
            },
            Self::CodeBlock { language, text } => {
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
            Self::Quote { blocks } => {
                let mut inner = String::new();
                for block in blocks {
                    block.write_gemini(&mut inner);
                }
                for line in inner.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
            },
            Self::List { items, .. } => {
                for item in items {
                    let mut inner = String::new();
                    for block in item {
                        block.write_gemini(&mut inner);
                    }
                    let trimmed = inner.trim();
                    out.push_str("* ");
                    out.push_str(trimmed);
                    out.push('\n');
                }
            },
            Self::Image { url, alt } => {
                out.push_str("=> ");
                out.push_str(url);
                if !alt.is_empty() {
                    out.push(' ');
                    out.push_str(alt);
                }
                out.push('\n');
            },
            Self::Preformatted { text } => {
                out.push_str("```\n");
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            },
            Self::Rule => out.push('\n'),
            Self::FeedHeader {
                title,
                subtitle,
                summary,
                source_url,
            } => {
                out.push_str(&format!("# {title}\n"));
                if let Some(subtitle) = subtitle {
                    out.push_str(&format!("## {subtitle}\n"));
                }
                if let Some(summary) = summary {
                    out.push_str(summary);
                    out.push('\n');
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("=> {url} Open source\n"));
                }
            },
            Self::FeedEntry {
                title,
                date,
                summary,
                article_url,
                source_url,
            } => {
                out.push_str(&format!("## {title}\n"));
                if let Some(date) = date {
                    out.push_str(&format!("> {date}\n"));
                }
                if let Some(summary) = summary {
                    out.push_str(summary);
                    out.push('\n');
                }
                if let Some(url) = article_url {
                    out.push_str(&format!("=> {url} Open article\n"));
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("=> {url} Open source\n"));
                }
            },
            Self::MetadataRow { label, value } => {
                out.push_str(&format!("{label}: {value}\n"));
            },
            Self::Badge { text } => {
                out.push_str(&format!("> {text}\n"));
            },
        }
    }
}

impl Block {
    /// Knot-format renderer. Reuses `write_markdown` for structural blocks;
    /// emits fenced code blocks with protocol language tags for the four
    /// semantic block variants so they round-trip through
    /// `nematic::KnotEngine`'s fence expansion.
    fn write_knot(&self, out: &mut String) {
        match self {
            Self::FeedHeader {
                title,
                subtitle,
                summary,
                source_url,
            } => {
                out.push_str("```feed-header\n");
                out.push_str(&format!("title: {title}\n"));
                if let Some(s) = subtitle {
                    out.push_str(&format!("subtitle: {s}\n"));
                }
                if let Some(s) = summary {
                    out.push_str(&format!("summary: {s}\n"));
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("source: {url}\n"));
                }
                out.push_str("```\n\n");
            },
            Self::FeedEntry {
                title,
                date,
                summary,
                article_url,
                source_url,
            } => {
                out.push_str("```feed-entry\n");
                out.push_str(&format!("title: {title}\n"));
                if let Some(d) = date {
                    out.push_str(&format!("date: {d}\n"));
                }
                if let Some(s) = summary {
                    out.push_str(&format!("summary: {s}\n"));
                }
                if let Some(url) = article_url {
                    out.push_str(&format!("url: {url}\n"));
                }
                if let Some(url) = source_url {
                    out.push_str(&format!("source: {url}\n"));
                }
                out.push_str("```\n\n");
            },
            Self::MetadataRow { label, value } => {
                out.push_str("```metadata-row\n");
                out.push_str(&format!("{label}: {value}\n"));
                out.push_str("```\n\n");
            },
            Self::Badge { text } => {
                out.push_str("```badge\n");
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n\n");
            },
            // Recurse into containers so semantic blocks nested inside
            // quotes / list items also serialise as fences.
            Self::Quote { blocks } => {
                let mut inner = String::new();
                for block in blocks {
                    block.write_knot(&mut inner);
                }
                for line in inner.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            },
            Self::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        out.push_str(&format!("{}. ", i + 1));
                    } else {
                        out.push_str("- ");
                    }
                    let mut inner = String::new();
                    for block in item {
                        block.write_knot(&mut inner);
                    }
                    out.push_str(inner.trim_end());
                    out.push('\n');
                }
                out.push('\n');
            },
            // For everything else, knot output and markdown output match.
            other => other.write_markdown(out, 0),
        }
    }
}

fn write_inline_markdown(spans: &[InlineSpan], out: &mut String) {
    for span in spans {
        match span {
            InlineSpan::Text(t) => out.push_str(t),
            InlineSpan::Code(t) => {
                out.push('`');
                out.push_str(t);
                out.push('`');
            },
            InlineSpan::Emphasis(s) => {
                out.push('*');
                write_inline_markdown(s, out);
                out.push('*');
            },
            InlineSpan::Strong(s) => {
                out.push_str("**");
                write_inline_markdown(s, out);
                out.push_str("**");
            },
            InlineSpan::Link { url, spans, .. } => {
                out.push('[');
                write_inline_markdown(spans, out);
                out.push_str("](");
                out.push_str(url);
                out.push(')');
            },
            InlineSpan::SoftBreak => out.push('\n'),
            InlineSpan::LineBreak => out.push_str("  \n"),
        }
    }
}

/// Whether a span list is links and whitespace only (with at least one
/// link) — the "this paragraph IS a link line" case shared by the gemtext
/// and gophermap writers.
fn is_link_only(spans: &[InlineSpan]) -> bool {
    let mut saw_link = false;
    for span in spans {
        match span {
            InlineSpan::Link { .. } => saw_link = true,
            InlineSpan::Text(text) if text.trim().is_empty() => {},
            InlineSpan::SoftBreak | InlineSpan::LineBreak => {},
            _ => return false,
        }
    }
    saw_link
}

fn collect_link_targets(span: &InlineSpan, out: &mut Vec<(String, String)>) {
    match span {
        InlineSpan::Link { url, spans, .. } => {
            out.push((url.clone(), inline_text(spans)));
            for inner in spans {
                collect_link_targets(inner, out);
            }
        },
        InlineSpan::Emphasis(spans) | InlineSpan::Strong(spans) => {
            for inner in spans {
                collect_link_targets(inner, out);
            }
        },
        _ => {},
    }
}

// Gophermap + plain-text exporters live in `render/export.rs` (this file is
// at the 600-LOC ceiling); tests live in `render/tests.rs`.
mod export;
pub use export::GophermapContext;

// Tests live in `render/tests.rs` to keep this file under the 600-LOC ceiling.
#[cfg(test)]
mod tests;
