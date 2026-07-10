/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Gophermap + plain-text exporters — the rest of the export set beside
//! `to_markdown` / `to_gemini` / `to_knot` (the knot evaluation + export
//! plan's K5).
//!
//! Downgrade rules, documented where they bite:
//!
//! - **Gophermap** (RFC 1436 menu): prose becomes `i` info lines (selector
//!   `fake`, host `(NULL)`, port `0` — the de-facto convention clients
//!   expect for non-selectable text). Links become selectable items:
//!   `gopher://` URLs are decomposed into real `1`-type menu entries on
//!   their own host; every other scheme becomes the `h`-type
//!   `URL:<url>` form, served back through the exporting server (which is
//!   why [`GophermapContext`] carries host + port). Inline styling
//!   flattens; long lines are not wrapped (modern clients cope; the
//!   classic 67-column rule is the caller's typography decision). The map
//!   ends with the `.` terminator line.
//! - **Plain text**: structure flattens to readable text — headings keep a
//!   blank line around them, list items take `- ` / `N. ` markers, quotes
//!   indent with `> `, links render as `label <url>`, everything else is
//!   its text.

use super::super::{Block, EngineDocument, InlineSpan, inline_text};

/// Server context a gophermap is written against: the host/port columns
/// RFC 1436 requires on every menu line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GophermapContext {
    pub host: String,
    pub port: u16,
}

impl EngineDocument {
    /// Render the document as a gophermap (RFC 1436 menu). See the module
    /// docs for the downgrade rules.
    pub fn to_gophermap(&self, ctx: &GophermapContext) -> String {
        // Like the sibling exporters, blocks only — the title lives in
        // frontmatter (to_knot) or the document's own heading, never
        // invented here.
        let mut out = String::new();
        for block in &self.blocks {
            write_gophermap_block(block, ctx, &mut out, "");
        }
        out.push_str(".\r\n");
        out
    }

    /// Render the document as plain text. See the module docs for the
    /// flattening rules.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            write_text_block(block, &mut out, "");
        }
        out
    }
}

/// One non-selectable info line. The `fake`/`(NULL)`/`0` columns are the
/// convention gopher clients treat as "display only".
fn push_info(out: &mut String, text: &str) {
    for line in if text.is_empty() { "\u{0}" } else { text }.split('\n') {
        let line = if line == "\u{0}" { "" } else { line };
        out.push('i');
        out.push_str(line);
        out.push_str("\tfake\t(NULL)\t0\r\n");
    }
}

/// One selectable line for a URL: real gopher links decompose into native
/// menu entries; everything else uses the `URL:` redirect convention served
/// by `ctx`'s host.
fn push_link(out: &mut String, ctx: &GophermapContext, url: &str, label: &str) {
    let display = if label.is_empty() { url } else { label };
    if let Some(rest) = url.strip_prefix("gopher://") {
        // gopher://host[:port]/<type><selector>
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match hostport.split_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().unwrap_or(70)),
            None => (hostport, 70),
        };
        let mut chars = path.chars();
        let item_type = chars.next().unwrap_or('1');
        // The selector keeps whatever follows the type character verbatim
        // (it usually carries its own leading slash, e.g. `/1/gopher`).
        let selector: String = chars.collect();
        out.push(item_type);
        out.push_str(display);
        out.push('\t');
        out.push_str(if selector.is_empty() { "/" } else { &selector });
        out.push('\t');
        out.push_str(host);
        out.push_str(&format!("\t{port}\r\n"));
    } else {
        out.push('h');
        out.push_str(display);
        out.push_str(&format!("\tURL:{url}\t{}\t{}\r\n", ctx.host, ctx.port));
    }
}

fn collect_links(span: &InlineSpan, out: &mut Vec<(String, String)>) {
    match span {
        InlineSpan::Link { url, spans, .. } => {
            out.push((url.clone(), inline_text(spans)));
            for inner in spans {
                collect_links(inner, out);
            }
        }
        InlineSpan::Emphasis(spans) | InlineSpan::Strong(spans) => {
            for inner in spans {
                collect_links(inner, out);
            }
        }
        _ => {}
    }
}

fn write_gophermap_block(block: &Block, ctx: &GophermapContext, out: &mut String, prefix: &str) {
    match block {
        Block::Heading { spans, .. } => {
            push_info(out, &format!("{prefix}{}", inline_text(spans)));
            push_info(out, "");
        }
        Block::Paragraph { spans } => {
            // A link-only paragraph IS its link line (same rule as the
            // gemtext writer) — no doubled info line.
            let text = inline_text(spans);
            if !text.is_empty() && !super::is_link_only(spans) {
                push_info(out, &format!("{prefix}{text}"));
            }
            let mut links = Vec::new();
            for span in spans {
                collect_links(span, &mut links);
            }
            for (url, label) in links {
                push_link(out, ctx, &url, &label);
            }
        }
        Block::CodeBlock { text, .. } | Block::Preformatted { text } => {
            for line in text.lines() {
                push_info(out, &format!("{prefix}{line}"));
            }
        }
        Block::Quote { blocks } => {
            for inner in blocks {
                write_gophermap_block(inner, ctx, out, &format!("{prefix}> "));
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for inner in item {
                    write_gophermap_block(inner, ctx, out, &format!("{prefix}* "));
                }
            }
        }
        Block::Image { url, alt } => push_link(out, ctx, url, alt),
        Block::Rule => push_info(out, &format!("{prefix}---")),
        Block::FeedHeader {
            title, subtitle, ..
        } => {
            push_info(out, &format!("{prefix}{title}"));
            if let Some(subtitle) = subtitle {
                push_info(out, &format!("{prefix}{subtitle}"));
            }
        }
        Block::FeedEntry {
            title,
            date,
            article_url,
            ..
        } => {
            let dated = match date {
                Some(date) => format!("{prefix}{title} — {date}"),
                None => format!("{prefix}{title}"),
            };
            match article_url {
                Some(url) => push_link(out, ctx, url, &dated),
                None => push_info(out, &dated),
            }
        }
        Block::MetadataRow { label, value } => {
            push_info(out, &format!("{prefix}{label}: {value}"));
        }
        Block::Badge { text } => push_info(out, &format!("{prefix}[{text}]")),
        Block::Table { header, rows, .. } => {
            for line in super::table_lines(header, rows) {
                push_info(out, &format!("{prefix}{line}"));
            }
        }
    }
}

fn write_text_block(block: &Block, out: &mut String, prefix: &str) {
    match block {
        Block::Table { header, rows, .. } => {
            for line in super::table_lines(header, rows) {
                out.push_str(prefix);
                out.push_str(&line);
                out.push('\n');
            }
            out.push('\n');
        }
        Block::Heading { spans, .. } => {
            out.push_str(prefix);
            out.push_str(&inline_text(spans));
            out.push_str("\n\n");
        }
        Block::Paragraph { spans } => {
            let text = text_with_links(spans);
            if !text.is_empty() {
                out.push_str(prefix);
                out.push_str(&text);
                out.push_str("\n\n");
            }
        }
        Block::CodeBlock { text, .. } | Block::Preformatted { text } => {
            for line in text.lines() {
                out.push_str(prefix);
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
        Block::Quote { blocks } => {
            for inner in blocks {
                write_text_block(inner, out, &format!("{prefix}> "));
            }
        }
        Block::List { ordered, items } => {
            for (index, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}. ", index + 1)
                } else {
                    "- ".to_string()
                };
                let mut inner = String::new();
                for block in item {
                    write_text_block(block, &mut inner, "");
                }
                out.push_str(prefix);
                out.push_str(&marker);
                out.push_str(inner.trim_end());
                out.push('\n');
            }
            out.push('\n');
        }
        Block::Image { url, alt } => {
            out.push_str(prefix);
            if alt.is_empty() {
                out.push_str(url);
            } else {
                out.push_str(&format!("{alt} <{url}>"));
            }
            out.push_str("\n\n");
        }
        Block::Rule => {
            out.push_str(prefix);
            out.push_str("---\n\n");
        }
        Block::FeedHeader {
            title, subtitle, ..
        } => {
            out.push_str(prefix);
            out.push_str(title);
            out.push('\n');
            if let Some(subtitle) = subtitle {
                out.push_str(prefix);
                out.push_str(subtitle);
                out.push('\n');
            }
            out.push('\n');
        }
        Block::FeedEntry {
            title,
            date,
            summary,
            article_url,
            ..
        } => {
            out.push_str(prefix);
            match date {
                Some(date) => out.push_str(&format!("{title} — {date}\n")),
                None => {
                    out.push_str(title);
                    out.push('\n');
                }
            }
            if let Some(summary) = summary {
                out.push_str(prefix);
                out.push_str(summary);
                out.push('\n');
            }
            if let Some(url) = article_url {
                out.push_str(prefix);
                out.push('<');
                out.push_str(url);
                out.push_str(">\n");
            }
            out.push('\n');
        }
        Block::MetadataRow { label, value } => {
            out.push_str(prefix);
            out.push_str(&format!("{label}: {value}\n"));
        }
        Block::Badge { text } => {
            out.push_str(prefix);
            out.push_str(&format!("[{text}]\n"));
        }
    }
}

/// Paragraph text where each link renders as `label <url>` in place.
fn text_with_links(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        match span {
            InlineSpan::Link { url, spans, .. } => {
                let label = inline_text(spans);
                if label.is_empty() || label == *url {
                    out.push('<');
                    out.push_str(url);
                    out.push('>');
                } else {
                    out.push_str(&format!("{label} <{url}>"));
                }
            }
            InlineSpan::Emphasis(spans) | InlineSpan::Strong(spans) => {
                out.push_str(&text_with_links(spans));
            }
            InlineSpan::Text(text) | InlineSpan::Code(text) => out.push_str(text),
            InlineSpan::SoftBreak => out.push(' '),
            InlineSpan::LineBreak => out.push('\n'),
        }
    }
    out
}
