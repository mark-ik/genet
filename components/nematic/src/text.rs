/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Plain text engine.
//!
//! No structure beyond blank-line-separated paragraphs. Soft breaks within
//! paragraphs are preserved so the reader sees the original line shape.

use inker::{
    Block, DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError,
    EngineInput, InlineSpan,
};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.text";

/// Plain text engine.
pub struct TextEngine;

impl TextEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for TextEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let blocks: Vec<Block> = paragraphs(&input.body)
            .map(|para| Block::Paragraph {
                spans: paragraph_spans(para),
            })
            .collect();

        Ok(EngineDocument {
            address: input.address.clone(),
            title: None,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| "text/plain".to_string()),
            lang: None,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        })
    }
}

fn paragraphs(body: &str) -> impl Iterator<Item = &str> {
    body.split("\n\n")
        .map(str::trim_end)
        .filter(|para| !para.is_empty())
}

fn paragraph_spans(para: &str) -> Vec<InlineSpan> {
    let mut spans = Vec::new();
    let mut first = true;
    for line in para.lines() {
        if !first {
            spans.push(InlineSpan::SoftBreak);
        }
        spans.push(InlineSpan::Text(line.to_string()));
        first = false;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        TextEngine::new()
            .render(&EngineInput::new("text:test", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(TextEngine::new().engine_id(), "nematic.text");
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        let doc = render("hello\n\nworld\n");
        assert_eq!(doc.blocks.len(), 2);
        assert!(matches!(doc.blocks[0], Block::Paragraph { .. }));
        assert!(matches!(doc.blocks[1], Block::Paragraph { .. }));
    }

    #[test]
    fn soft_breaks_preserved_within_paragraph() {
        let doc = render("alpha\nbeta\ngamma\n");
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let breaks = spans
            .iter()
            .filter(|s| matches!(s, InlineSpan::SoftBreak))
            .count();
        assert_eq!(breaks, 2);
    }

    #[test]
    fn no_title_extracted() {
        let doc = render("first line\n\nsecond paragraph");
        assert!(doc.title.is_none());
    }

    #[test]
    fn empty_body_yields_no_blocks() {
        let doc = render("");
        assert!(doc.blocks.is_empty());
    }

    #[test]
    fn input_content_type_overrides_default() {
        let doc = TextEngine::new()
            .render(&EngineInput::new("text:1", "hi").with_content_type("text/x-custom"))
            .expect("render");
        assert_eq!(doc.content_type, "text/x-custom");
    }
}
