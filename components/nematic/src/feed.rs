/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Feed engine — RSS 2.0, Atom 1.0, and JSON Feed 1.x into a portable document.
//!
//! RSS / Atom (XML) parsing lives in [`errand::parse::feed`] (shared with the
//! native genet feed view); JSON Feed is parsed here with serde, since errand (a
//! transport crate) should not carry a JSON dependency. Both flavours land in
//! errand's [`Feed`] shape, so they share [`build_document_blocks`].
//!
//! The output layout: the feed title becomes [`EngineDocument::title`]; the feed
//! emits one [`Block::FeedHeader`] (when it carries channel metadata) then
//! one [`Block::FeedEntry`] per item. Summaries are de-tagged to plain
//! text (lossy v1), and the count of stripped entries surfaces as a
//! `DegradedRendering` diagnostic.

use errand::parse::feed::{Feed, FeedEntry, parse as parse_feed_xml, strip_html_tags};
use inker::{
    Block, DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput,
};
use serde::Deserialize;

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.feed";

/// RSS / Atom / JSON Feed engine.
pub struct FeedEngine;

impl FeedEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FeedEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for FeedEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let is_json = looks_like_json_feed(input);
        let feed = if is_json {
            parse_json(&input.body)?
        } else {
            parse_feed_xml(&input.body).map_err(|e| EngineError::InvalidContent(e.to_string()))?
        };
        let title = feed.title.clone();
        let lang = feed.lang.clone();
        let (blocks, diagnostics) = build_document_blocks(feed);

        let default_content_type = if is_json {
            "application/feed+json"
        } else {
            "application/feed+xml"
        };

        Ok(EngineDocument {
            address: input.address.clone(),
            title,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| default_content_type.to_string()),
            lang,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics,
            blocks,
        })
    }
}

/// Decide whether `input` is a JSON Feed rather than RSS/Atom XML. Prefers the
/// declared content type; an XML/RSS/Atom type short-circuits to the XML walker.
/// With no usable content type, sniff the body: a JSON Feed is an object, so the
/// first non-whitespace byte is `{`.
fn looks_like_json_feed(input: &EngineInput) -> bool {
    if let Some(content_type) = &input.content_type {
        let lowered = content_type.to_ascii_lowercase();
        if lowered.contains("json") {
            return true;
        }
        if lowered.contains("xml") || lowered.contains("rss") || lowered.contains("atom") {
            return false;
        }
    }
    input.body.trim_start().starts_with('{')
}

/// JSON Feed 1.x top-level object. Only the fields nematic projects are modelled;
/// unknown keys are ignored (forward-compatible with 1.1+).
#[derive(Deserialize)]
struct JsonFeed {
    title: Option<String>,
    home_page_url: Option<String>,
    feed_url: Option<String>,
    description: Option<String>,
    language: Option<String>,
    #[serde(default)]
    items: Vec<JsonFeedItem>,
}

#[derive(Deserialize)]
struct JsonFeedItem {
    id: Option<String>,
    url: Option<String>,
    external_url: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    content_text: Option<String>,
    content_html: Option<String>,
    date_published: Option<String>,
    date_modified: Option<String>,
}

/// Parse a JSON Feed 1.x document into errand's [`Feed`] shape, so it shares
/// [`build_document_blocks`] with the XML path.
fn parse_json(body: &str) -> Result<Feed, EngineError> {
    let feed: JsonFeed = serde_json::from_str(body)
        .map_err(|err| EngineError::InvalidContent(format!("JSON Feed parse failed: {err}")))?;

    let mut out = Feed {
        title: trimmed_some(feed.title),
        subtitle: trimmed_some(feed.description).map(|text| strip_html_tags(&text)),
        link: trimmed_some(feed.home_page_url).or_else(|| trimmed_some(feed.feed_url)),
        lang: trimmed_some(feed.language),
        ..Feed::default()
    };

    for item in feed.items {
        let title = trimmed_some(item.title).or_else(|| trimmed_some(item.id));
        let link = trimmed_some(item.url).or_else(|| trimmed_some(item.external_url));
        let date = trimmed_some(item.date_published).or_else(|| trimmed_some(item.date_modified));

        // Mirror the XML path: prefer summary, then plain-text content, then HTML
        // content; strip tags and count the strip for the degraded hint.
        let summary = trimmed_some(item.summary)
            .or_else(|| trimmed_some(item.content_text))
            .or_else(|| trimmed_some(item.content_html))
            .map(|raw| {
                if raw.contains('<') {
                    out.html_stripped += 1;
                    strip_html_tags(&raw)
                } else {
                    raw
                }
            });

        if title.is_some() || link.is_some() || summary.is_some() {
            out.entries.push(FeedEntry {
                title,
                link,
                date,
                summary,
            });
        }
    }

    Ok(out)
}

/// Trim a JSON string field, dropping it if empty after trimming.
fn trimmed_some(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Build the block list and any diagnostics from a parsed [`Feed`].
///
/// Emits one [`Block::FeedHeader`] when the feed carries channel-level
/// metadata, then one [`Block::FeedEntry`] per item. These semantic blocks
/// preserve RSS / Atom intent (a feed entry is distinct from "a paragraph with a
/// link in it") so downstream intelligence can match on type, not just text.
fn build_document_blocks(feed: Feed) -> (Vec<Block>, Vec<DocumentDiagnostic>) {
    let mut blocks = Vec::with_capacity(feed.entries.len() + 1);

    let header_has_content = feed.title.is_some() || feed.subtitle.is_some() || feed.link.is_some();
    if header_has_content {
        blocks.push(Block::FeedHeader {
            title: feed.title.clone().unwrap_or_default(),
            subtitle: feed.subtitle,
            summary: None,
            source_url: feed.link,
        });
    }

    for entry in feed.entries {
        blocks.push(Block::FeedEntry {
            title: entry.title.unwrap_or_default(),
            date: entry.date,
            summary: entry.summary,
            article_url: entry.link,
            source_url: None,
        });
    }

    let mut diagnostics = Vec::new();
    if feed.html_stripped > 0 {
        let n = feed.html_stripped;
        let entries = if n == 1 { "entry" } else { "entries" };
        diagnostics.push(DocumentDiagnostic::DegradedRendering(format!(
            "stripped HTML from {n} {entries}"
        )));
    }

    (blocks, diagnostics)
}

// Tests live in `feed/tests.rs` to keep this file under the 600-LOC ceiling.
#[cfg(test)]
mod tests;
