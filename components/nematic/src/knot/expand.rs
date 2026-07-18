/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Polyglot fence expansion + inline parsers + clip-knot builder.
//!
//! After the markdown engine produces an [`EngineDocument`] from a knot
//! body, [`expand_fenced_blocks`] walks the block list (recursing into
//! quotes and lists) and replaces any `CodeBlock` whose `language` is a
//! recognised protocol with the blocks that protocol's engine would
//! produce from the fence content.
//!
//! See `design_docs/nematic_docs/implementation_strategy/2026-05-08_polyglot_knot_design.md`.

use std::mem;

use inker::{
    Block, BlockProvenanceMap, DocumentProvenance, DocumentTrustState, Engine, EngineInput,
    InlineSpan,
};

use crate::{GemtextEngine, GopherEngine, NexEngine};

/// Walk `blocks`, expanding any fenced code block whose language tag is a
/// known protocol into real semantic blocks. Unknown languages pass
/// through unchanged.
pub(super) fn expand_fenced_blocks(blocks: &mut Vec<Block>) {
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for block in mem::take(blocks) {
        match block {
            Block::CodeBlock {
                language: Some(lang),
                text,
            } => match expand_fenced(&lang, &text) {
                Some(expanded) => out.extend(expanded),
                None => out.push(Block::CodeBlock {
                    language: Some(lang),
                    text,
                }),
            },
            Block::Quote { mut blocks } => {
                expand_fenced_blocks(&mut blocks);
                out.push(Block::Quote { blocks });
            },
            Block::List { ordered, items } => {
                let expanded_items = items
                    .into_iter()
                    .map(|mut item| {
                        expand_fenced_blocks(&mut item);
                        item
                    })
                    .collect();
                out.push(Block::List {
                    ordered,
                    items: expanded_items,
                });
            },
            other => out.push(other),
        }
    }
    *blocks = out;
}

/// Dispatch a single fence (`language`, `text`) to the right parser.
/// Returns `None` for languages this knot module doesn't recognise; the
/// caller keeps the original code block in that case.
fn expand_fenced(language: &str, text: &str) -> Option<Vec<Block>> {
    let lang = language.trim().to_ascii_lowercase();
    match lang.as_str() {
        "gemtext" => Some(parse_via_engine(&GemtextEngine::new(), text)),
        "gopher" => Some(parse_via_engine(&GopherEngine::new(), text)),
        "nex" => Some(parse_via_engine(&NexEngine::new(), text)),
        "feed-entry" => Some(vec![parse_feed_entry(text)]),
        "feed-header" => Some(vec![parse_feed_header(text)]),
        "metadata-row" | "metadata" => Some(parse_metadata_rows(text)),
        "badge" => Some(parse_badges(text)),
        _ => None,
    }
}

fn parse_via_engine(engine: &dyn Engine, text: &str) -> Vec<Block> {
    // The fence content has no canonical address of its own; pass an
    // opaque placeholder so the inner engine has something for its
    // provenance.canonical_uri.
    let input = EngineInput::new(format!("knot-fence:{}", engine.engine_id()), text);
    match engine.render(&input) {
        Ok(doc) => doc.blocks,
        Err(_) => Vec::new(),
    }
}

fn parse_feed_entry(text: &str) -> Block {
    let pairs = parse_kv_lines(text);
    let mut title = String::new();
    let mut date = None;
    let mut summary = None;
    let mut article_url = None;
    let mut source_url = None;
    let mut extras: Vec<Block> = Vec::new();

    for (key, value) in &pairs {
        match key.to_ascii_lowercase().as_str() {
            "title" => title = value.clone(),
            "date" | "pubdate" | "published" | "updated" => date = Some(value.clone()),
            "summary" | "description" | "content" => summary = Some(value.clone()),
            "url" | "article" | "link" => article_url = Some(value.clone()),
            "source" | "source_url" => source_url = Some(value.clone()),
            _ => extras.push(Block::MetadataRow {
                label: key.clone(),
                value: value.clone(),
            }),
        }
    }

    let entry = Block::FeedEntry {
        title,
        date,
        summary,
        article_url,
        source_url,
    };

    // If extras exist, emit them as sibling MetadataRows after the entry.
    // Caller flattens them in.
    if extras.is_empty() {
        entry
    } else {
        // Compress to a single Quote so the entry-plus-extras structure
        // stays grouped. Simpler: put extras in a Quote following the
        // entry. But callers expect a single block return. Fold extras
        // into a Quote that wraps the entry + extras.
        let mut grouped = vec![entry];
        grouped.extend(extras);
        Block::Quote { blocks: grouped }
    }
}

fn parse_feed_header(text: &str) -> Block {
    let pairs = parse_kv_lines(text);
    let mut title = String::new();
    let mut subtitle = None;
    let mut summary = None;
    let mut source_url = None;
    let mut extras: Vec<Block> = Vec::new();

    for (key, value) in &pairs {
        match key.to_ascii_lowercase().as_str() {
            "title" => title = value.clone(),
            "subtitle" => subtitle = Some(value.clone()),
            "summary" | "description" => summary = Some(value.clone()),
            "source" | "source_url" | "link" | "url" => source_url = Some(value.clone()),
            _ => extras.push(Block::MetadataRow {
                label: key.clone(),
                value: value.clone(),
            }),
        }
    }

    let header = Block::FeedHeader {
        title,
        subtitle,
        summary,
        source_url,
    };
    if extras.is_empty() {
        header
    } else {
        let mut grouped = vec![header];
        grouped.extend(extras);
        Block::Quote { blocks: grouped }
    }
}

fn parse_metadata_rows(text: &str) -> Vec<Block> {
    parse_kv_lines(text)
        .into_iter()
        .map(|(label, value)| Block::MetadataRow { label, value })
        .collect()
}

fn parse_badges(text: &str) -> Vec<Block> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Block::Badge {
            text: line.to_string(),
        })
        .collect()
}

fn parse_kv_lines(text: &str) -> Vec<(String, String)> {
    // Preserve original key case — metadata-row labels are user-facing.
    // Schema-matching parsers (parse_feed_entry / parse_feed_header) lowercase
    // their own comparison keys.
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            line.split_once(':')
                .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

// =============================================================================
// Inline wikilinks `[[name]]` + hashtags `#tag`
// =============================================================================

/// Walk all blocks and rewrite inline wikilinks / hashtags in-place.
///
/// - `[[node-name]]` → `InlineSpan::Link { url: "mere://node/<slug>", … }`
///   where `<slug>` is the name lowercased and spaces replaced with `-`.
///   The link's display text stays the original `node-name` so the surface
///   form is preserved.
/// - `#tag` → consumed from the text and emitted as a sibling
///   `Block::Badge { text: "#tag" }` appended after the
///   containing paragraph. Hashtags are extracted (not preserved inline)
///   so search / intelligence layers see them as semantic blocks.
pub(super) fn rewrite_inline_extensions(blocks: &mut Vec<Block>) {
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for block in mem::take(blocks) {
        match block {
            Block::Paragraph { spans } => {
                let (rewritten, hashtags) = rewrite_spans(spans);
                out.push(Block::Paragraph { spans: rewritten });
                for tag in hashtags {
                    out.push(Block::Badge { text: tag });
                }
            },
            Block::Heading { level, spans } => {
                // Hashtags inside headings aren't usually intended as tags;
                // leave wikilinks rewritten but don't extract hashtags.
                let (rewritten, _) = rewrite_spans_no_hashtags(spans);
                out.push(Block::Heading {
                    level,
                    spans: rewritten,
                });
            },
            Block::Quote { mut blocks } => {
                rewrite_inline_extensions(&mut blocks);
                out.push(Block::Quote { blocks });
            },
            Block::List { ordered, items } => {
                let rewritten_items = items
                    .into_iter()
                    .map(|mut item| {
                        rewrite_inline_extensions(&mut item);
                        item
                    })
                    .collect();
                out.push(Block::List {
                    ordered,
                    items: rewritten_items,
                });
            },
            other => out.push(other),
        }
    }
    *blocks = out;
}

/// Rewrite a span list: expand wikilinks inline, collect hashtags.
fn rewrite_spans(spans: Vec<InlineSpan>) -> (Vec<InlineSpan>, Vec<String>) {
    // pulldown-cmark splits punctuation like `[[` and `]]` into separate
    // Text spans; merge adjacent Text runs first so wikilink boundaries
    // are visible to the scanner.
    let merged = merge_adjacent_text(spans);
    let mut out = Vec::with_capacity(merged.len());
    let mut hashtags = Vec::new();
    for span in merged {
        rewrite_one_span(span, &mut out, &mut hashtags);
    }
    (out, hashtags)
}

/// Rewrite a span list expanding wikilinks but leaving hashtags as text.
fn rewrite_spans_no_hashtags(spans: Vec<InlineSpan>) -> (Vec<InlineSpan>, Vec<String>) {
    let merged = merge_adjacent_text(spans);
    let mut out = Vec::with_capacity(merged.len());
    let mut sink: Vec<String> = Vec::new();
    for span in merged {
        rewrite_one_span_keep_tags(span, &mut out, &mut sink);
    }
    (out, sink)
}

fn merge_adjacent_text(spans: Vec<InlineSpan>) -> Vec<InlineSpan> {
    let mut out: Vec<InlineSpan> = Vec::with_capacity(spans.len());
    let mut buffer = String::new();
    for span in spans {
        match span {
            InlineSpan::Text(text) => buffer.push_str(&text),
            other => {
                if !buffer.is_empty() {
                    out.push(InlineSpan::Text(mem::take(&mut buffer)));
                }
                out.push(other);
            },
        }
    }
    if !buffer.is_empty() {
        out.push(InlineSpan::Text(buffer));
    }
    out
}

fn rewrite_one_span(span: InlineSpan, out: &mut Vec<InlineSpan>, hashtags: &mut Vec<String>) {
    match span {
        InlineSpan::Text(text) => expand_text(&text, out, Some(hashtags)),
        InlineSpan::Emphasis(inner) => {
            let (rewritten, mut found) = rewrite_spans(inner);
            hashtags.append(&mut found);
            out.push(InlineSpan::Emphasis(rewritten));
        },
        InlineSpan::Strong(inner) => {
            let (rewritten, mut found) = rewrite_spans(inner);
            hashtags.append(&mut found);
            out.push(InlineSpan::Strong(rewritten));
        },
        InlineSpan::Link {
            url,
            title,
            spans: inner,
            ..
        } => {
            // Don't rewrite anything inside an existing link — its display
            // text is already linked. Pass through verbatim.
            out.push(InlineSpan::Link {
                url,
                title,
                spans: inner,
                predicate: None,
            });
        },
        other => out.push(other),
    }
}

fn rewrite_one_span_keep_tags(
    span: InlineSpan,
    out: &mut Vec<InlineSpan>,
    _ignored: &mut Vec<String>,
) {
    match span {
        InlineSpan::Text(text) => expand_text(&text, out, None),
        InlineSpan::Emphasis(inner) => {
            let (rewritten, _) = rewrite_spans_no_hashtags(inner);
            out.push(InlineSpan::Emphasis(rewritten));
        },
        InlineSpan::Strong(inner) => {
            let (rewritten, _) = rewrite_spans_no_hashtags(inner);
            out.push(InlineSpan::Strong(rewritten));
        },
        InlineSpan::Link { .. } => out.push(span),
        other => out.push(other),
    }
}

/// Scan a text run for `[[name]]` wikilinks and `#tag` hashtags. Emits
/// rewritten spans into `out`. When `hashtags` is `Some`, hashtag tokens
/// are extracted (not kept as text) and appended to the vec; when `None`,
/// hashtags are left as plain text.
fn expand_text(text: &str, out: &mut Vec<InlineSpan>, hashtags: Option<&mut Vec<String>>) {
    let mut buffer = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut maybe_hashtags = hashtags;

    while i < text.len() {
        // Wikilink: `[[...]]`
        if i + 4 <= text.len() && &text[i..i + 2] == "[[" {
            if let Some(end) = text[i + 2..].find("]]") {
                let inner = &text[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    flush_buffer(&mut buffer, out);
                    let display = inner.trim().to_string();
                    let url = wikilink_url(&display);
                    out.push(InlineSpan::Link {
                        url,
                        title: None,
                        spans: vec![InlineSpan::Text(display)],
                        predicate: None,
                    });
                    i += 2 + end + 2;
                    continue;
                }
            }
        }

        // Hashtag: `#word` where `#` is at start-of-string or preceded by
        // whitespace, and `word` is alphanumeric/underscore/dash, len ≥ 1.
        if bytes[i] == b'#' && (i == 0 || is_hashtag_boundary(bytes[i - 1])) {
            let tag_start = i + 1;
            let mut tag_end = tag_start;
            while tag_end < text.len() && is_hashtag_char(bytes[tag_end]) {
                tag_end += 1;
            }
            if tag_end > tag_start {
                let tag = &text[i..tag_end];
                if let Some(sink) = maybe_hashtags.as_mut() {
                    flush_buffer(&mut buffer, out);
                    sink.push(tag.to_string());
                    i = tag_end;
                    continue;
                }
            }
        }

        // Push one char's worth of bytes. Advance by char boundary length so
        // we don't split a multi-byte UTF-8 codepoint.
        let ch = text[i..].chars().next().expect("non-empty remaining text");
        buffer.push(ch);
        i += ch.len_utf8();
    }
    flush_buffer(&mut buffer, out);
}

fn flush_buffer(buffer: &mut String, out: &mut Vec<InlineSpan>) {
    if !buffer.is_empty() {
        out.push(InlineSpan::Text(mem::take(buffer)));
    }
}

fn wikilink_url(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    format!("mere://node/{slug}")
}

fn is_hashtag_boundary(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'\t' | b'\n' | b'(' | b'[' | b',' | b'.' | b';'
    )
}

fn is_hashtag_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

// =============================================================================
// build_clip_knot — assemble a knot file from raw blocks + provenance
// =============================================================================

mod build;
pub use build::{build_clip_knot, build_clip_knot_with_block_provenance};
