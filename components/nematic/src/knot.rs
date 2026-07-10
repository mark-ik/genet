/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Knot — Mere's native note / clip format.
//!
//! A *knot* is a graph-native unit of remembered content: a clip from a
//! webpage, a quote with a citation, a thought of one's own, a summary of
//! something else. The format pairs YAML-shaped frontmatter (carrying the
//! note's *meaning* — provenance, trust, kind, tags) with a markdown body
//! (carrying the *content*). Frontmatter populates [`EngineDocument`]'s
//! semantic fields (`provenance`, `trust`); body parses through the
//! markdown engine.
//!
//! ```text
//! ---
//! title: My research note
//! source: https://example.com/article
//! captured: 2026-05-08T14:23:00Z
//! trust: tofu
//! tags: [research, semantics, mere]
//! note_kind: clip
//! ---
//!
//! # Body content
//!
//! Markdown body...
//! ```
//!
//! Unlike the protocol engines (gemini, gopher, RSS, finger), knot is a
//! file format that Mere defines, so it can carry richer semantic content
//! without violating any external spec. This is the lane that fuels
//! intelligence over notes / clips: every knot carries explicit
//! provenance, trust state, and kind, so downstream search / summarise /
//! recall can match on meaning, not just text.
//!
//! Frontmatter parsing is a deliberately small YAML subset:
//!
//! - `key: value` — string scalar
//! - `key: [a, b, c]` — flow-style string array
//!
//! Quoted strings, multi-line literals, nested mappings, and anchors are
//! out of scope for v1. A real YAML dependency is unwarranted for a
//! flat key-value frontmatter.

use std::collections::HashMap;

use inker::{
    Block, DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError, EngineInput,
};

use crate::MarkdownEngine;

/// Djot-substrate proof-of-concept (design doc §10, Phase 1). Additive and not
/// yet wired into the engine; parses a djot knot body into `Block`s.
pub mod djot;
mod expand;
#[cfg(test)]
mod tests;

pub use expand::{build_clip_knot, build_clip_knot_with_block_provenance};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.knot";

/// Knot note / clip engine. Owns a markdown engine for body parsing.
pub struct KnotEngine {
    markdown: MarkdownEngine,
}

impl KnotEngine {
    pub fn new() -> Self {
        Self {
            markdown: MarkdownEngine::new(),
        }
    }
}

impl Default for KnotEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for KnotEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let (frontmatter, body) = split_frontmatter(&input.body);

        let body_input = EngineInput {
            address: input.address.clone(),
            body: body.to_string(),
            content_type: Some("text/markdown".to_string()),
        };
        let mut doc = self.markdown.render(&body_input)?;

        // Polyglot expansion: walk the markdown engine's blocks and expand
        // any fenced code blocks whose language tag matches a known
        // protocol (gemtext / gopher / nex / feed-entry / feed-header /
        // metadata-row / badge) into real `Block`s. Unknown
        // languages stay as code blocks.
        expand::expand_fenced_blocks(&mut doc.blocks);

        // Inline wikilinks `[[name]]` and hashtags `#tag` get rewritten in
        // place: wikilinks become `mere://node/<name>` Link spans so they
        // route through graphshell.internal; hashtags emit one Badge block
        // per tag at the end of their containing paragraph.
        expand::rewrite_inline_extensions(&mut doc.blocks);

        apply_frontmatter(
            &mut doc,
            &frontmatter,
            self.engine_id(),
            &input.address,
            input.content_type.as_deref(),
        );

        Ok(doc)
    }
}

/// Apply a knot's frontmatter to a rendered document: title override,
/// provenance (engine id + source / captured / label), trust state,
/// content-type, and prefix `MetadataRow`s for `note_kind` / `tags`. Shared by
/// the CommonMark [`KnotEngine`] and the experimental [`djot::DjotKnotEngine`]
/// so both knot grammars carry identical note semantics.
fn apply_frontmatter(
    doc: &mut EngineDocument,
    frontmatter: &HashMap<String, FrontmatterValue>,
    engine_id: &str,
    address: &str,
    content_type: Option<&str>,
) {
    if let Some(FrontmatterValue::Scalar(title)) = frontmatter.get("title") {
        doc.title = Some(title.clone());
    }

    let mut provenance = DocumentProvenance::for_engine(engine_id, address);
    if let Some(FrontmatterValue::Scalar(source)) = frontmatter.get("source") {
        provenance.canonical_uri = Some(source.clone());
    }
    if let Some(FrontmatterValue::Scalar(captured)) = frontmatter.get("captured") {
        provenance.fetched_at = Some(captured.clone());
    }
    if let Some(FrontmatterValue::Scalar(label)) = frontmatter.get("source_label") {
        provenance.source_label = Some(label.clone());
    }
    doc.provenance = provenance;

    if let Some(FrontmatterValue::Scalar(trust)) = frontmatter.get("trust") {
        doc.trust = parse_trust(trust);
    }

    doc.content_type = content_type
        .map(|s| s.to_string())
        .unwrap_or_else(|| "text/x-knot".to_string());

    let mut prefix: Vec<Block> = Vec::new();
    if let Some(FrontmatterValue::Scalar(kind)) = frontmatter.get("note_kind") {
        prefix.push(Block::MetadataRow {
            label: "kind".to_string(),
            value: kind.clone(),
        });
    }
    if let Some(value) = frontmatter.get("tags") {
        let joined = match value {
            FrontmatterValue::Scalar(s) => s.clone(),
            FrontmatterValue::List(items) => items.join(", "),
        };
        if !joined.is_empty() {
            prefix.push(Block::MetadataRow {
                label: "tags".to_string(),
                value: joined,
            });
        }
    }
    if !prefix.is_empty() {
        prefix.append(&mut doc.blocks);
        doc.blocks = prefix;
    }
}

#[derive(Clone, Debug)]
enum FrontmatterValue {
    Scalar(String),
    List(Vec<String>),
}

fn split_frontmatter(input: &str) -> (HashMap<String, FrontmatterValue>, &str) {
    // Frontmatter requires a leading `---` line at byte 0.
    let opener_len = if let Some(rest) = input.strip_prefix("---\n") {
        input.len() - rest.len()
    } else if let Some(rest) = input.strip_prefix("---\r\n") {
        input.len() - rest.len()
    } else {
        return (HashMap::new(), input);
    };

    let after_opener = &input[opener_len..];

    // Find a closing `---` line.
    let close_match = find_close_marker(after_opener);
    let Some((close_start, close_end)) = close_match else {
        // No closing marker — treat the whole input as body so we don't
        // silently swallow content into an unterminated frontmatter.
        return (HashMap::new(), input);
    };

    let yaml = &after_opener[..close_start];
    let body = &after_opener[close_end..];
    let map = parse_frontmatter_yaml(yaml);
    (map, body)
}

fn find_close_marker(haystack: &str) -> Option<(usize, usize)> {
    // Scan line-by-line; the closer is a line whose only content is `---`.
    let mut idx = 0;
    while idx < haystack.len() {
        let rest = &haystack[idx..];
        let line_end = rest
            .find('\n')
            .map(|n| idx + n + 1)
            .unwrap_or(haystack.len());
        let line = haystack[idx..line_end].trim_end_matches(['\n', '\r']);
        if line == "---" {
            return Some((idx, line_end));
        }
        idx = line_end;
    }
    None
}

fn parse_frontmatter_yaml(yaml: &str) -> HashMap<String, FrontmatterValue> {
    let mut map = HashMap::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }

        let parsed = if value.starts_with('[') && value.ends_with(']') && value.len() >= 2 {
            let inner = &value[1..value.len() - 1];
            let items: Vec<String> = inner
                .split(',')
                .map(|item| strip_optional_quotes(item.trim()).to_string())
                .filter(|item| !item.is_empty())
                .collect();
            FrontmatterValue::List(items)
        } else {
            FrontmatterValue::Scalar(strip_optional_quotes(value).to_string())
        };

        map.insert(key.to_string(), parsed);
    }
    map
}

fn strip_optional_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn parse_trust(value: &str) -> DocumentTrustState {
    match value.trim().to_ascii_lowercase().as_str() {
        "trusted" => DocumentTrustState::Trusted,
        "tofu" => DocumentTrustState::Tofu,
        "insecure" => DocumentTrustState::Insecure,
        "broken" => DocumentTrustState::Broken,
        _ => DocumentTrustState::Unknown,
    }
}
