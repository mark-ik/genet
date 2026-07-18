/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Gemtext engine — Gemini's `text/gemini` content type.
//!
//! Line-oriented format. Each line's prefix decides its block type; lines
//! without a prefix accumulate into a paragraph until interrupted.
//!
//! - `# `, `## `, `### ` → headings (levels 1–3)
//! - `=> URL [whitespace] [text]` → link line
//! - `* ` → list item (consecutive items merge into one list)
//! - `> ` → quote line (consecutive lines merge into one quote)
//! - ` ``` ` (fence) toggles a preformatted block; an alt-text after the
//!   opening fence is captured as the block's language hint
//! - blank lines flush the current paragraph but leave list/quote runs intact
//!   if the next non-blank line continues them
//!
//! References: <https://gemini.circumlunar.space/docs/specification.html>.

use errand::parse::gemtext::{GemLine, parse as parse_gemtext};
use inker::{
    Block, DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError,
    EngineInput, InlineSpan,
};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.gemtext";

/// Gemtext engine.
pub struct GemtextEngine;

impl GemtextEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GemtextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for GemtextEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        // errand owns the gemtext grammar (line-level AST); nematic owns only the
        // grouping into the `Block` model (paragraph runs, list/quote merging).
        let lines = parse_gemtext(&input.body);
        let (blocks, title) = Lowering::run(&lines);

        Ok(EngineDocument {
            address: input.address.clone(),
            title,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| "text/gemini".to_string()),
            // Gemtext has no in-band language declaration.
            lang: None,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        })
    }
}

/// Lowers a gemtext [`GemLine`] stream into `Block`s, grouping consecutive
/// text lines into a paragraph, `* ` items into one list, and `> ` lines into one
/// quote — the model decisions errand's line-level parser deliberately leaves open.
#[derive(Default)]
struct Lowering {
    blocks: Vec<Block>,
    title: Option<String>,
    pending: Pending,
}

#[derive(Default)]
enum Pending {
    #[default]
    None,
    Paragraph(Vec<String>),
    List(Vec<Vec<Block>>),
    Quote(Vec<String>),
}

impl Lowering {
    fn run(lines: &[GemLine]) -> (Vec<Block>, Option<String>) {
        let mut state = Self::default();
        for line in lines {
            state.handle(line);
        }
        state.flush_pending();
        (state.blocks, state.title)
    }

    fn handle(&mut self, line: &GemLine) {
        match line {
            GemLine::Heading { level, text } => {
                self.flush_pending();
                self.push_heading(*level, text);
            },
            GemLine::Link { url, label } => {
                self.flush_pending();
                self.push_link(url, label);
            },
            GemLine::Pre { alt, text } => {
                self.flush_pending();
                self.blocks.push(Block::CodeBlock {
                    language: alt.clone(),
                    text: text.clone(),
                });
            },
            GemLine::Item(text) => {
                if !matches!(self.pending, Pending::List(_)) {
                    self.flush_pending();
                    self.pending = Pending::List(Vec::new());
                }
                if let Pending::List(items) = &mut self.pending {
                    items.push(vec![Block::Paragraph {
                        spans: vec![InlineSpan::Text(text.clone())],
                    }]);
                }
            },
            GemLine::Quote(text) => {
                if !matches!(self.pending, Pending::Quote(_)) {
                    self.flush_pending();
                    self.pending = Pending::Quote(Vec::new());
                }
                if let Pending::Quote(lines) = &mut self.pending {
                    lines.push(text.clone());
                }
            },
            GemLine::Text(text) => {
                if !matches!(self.pending, Pending::Paragraph(_)) {
                    self.flush_pending();
                    self.pending = Pending::Paragraph(Vec::new());
                }
                if let Pending::Paragraph(lines) = &mut self.pending {
                    lines.push(text.clone());
                }
            },
            GemLine::Blank => {
                // Blank lines flush paragraphs but keep list/quote runs alive across
                // single blank separators (gemtext readers commonly accept this).
                if matches!(self.pending, Pending::Paragraph(_)) {
                    self.flush_pending();
                }
            },
        }
    }

    fn push_heading(&mut self, level: u8, text: &str) {
        // The parser already trimmed the heading text.
        if level == 1 && self.title.is_none() && !text.is_empty() {
            self.title = Some(text.to_string());
        }
        self.blocks.push(Block::Heading {
            level,
            spans: vec![InlineSpan::Text(text.to_string())],
        });
    }

    fn push_link(&mut self, url: &str, label: &str) {
        let display = if label.is_empty() { url } else { label };
        self.blocks.push(Block::Paragraph {
            spans: vec![InlineSpan::Link {
                url: url.to_string(),
                title: None,
                spans: vec![InlineSpan::Text(display.to_string())],
                predicate: None,
            }],
        });
    }

    fn flush_pending(&mut self) {
        match std::mem::take(&mut self.pending) {
            Pending::None => {},
            Pending::Paragraph(lines) => {
                if !lines.is_empty() {
                    self.blocks.push(Block::Paragraph {
                        spans: join_soft(lines),
                    });
                }
            },
            Pending::List(items) => {
                if !items.is_empty() {
                    self.blocks.push(Block::List {
                        ordered: false,
                        items,
                    });
                }
            },
            Pending::Quote(lines) => {
                if !lines.is_empty() {
                    self.blocks.push(Block::Quote {
                        blocks: vec![Block::Paragraph {
                            spans: join_soft(lines),
                        }],
                    });
                }
            },
        }
    }
}

/// Join lines into paragraph spans, separated by a soft break (matching gemtext's
/// "wrapped text lines are one paragraph" reading).
fn join_soft(lines: Vec<String>) -> Vec<InlineSpan> {
    let mut spans = Vec::with_capacity(lines.len() * 2);
    let mut first = true;
    for line in lines {
        if !first {
            spans.push(InlineSpan::SoftBreak);
        }
        spans.push(InlineSpan::Text(line));
        first = false;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        GemtextEngine::new()
            .render(&EngineInput::new("gemini://test/", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(GemtextEngine::new().engine_id(), "nematic.gemtext");
    }

    #[test]
    fn h1_becomes_title() {
        let doc = render("# Welcome\n\nHello.\n");
        assert_eq!(doc.title.as_deref(), Some("Welcome"));
        assert_eq!(doc.content_type, "text/gemini");
    }

    #[test]
    fn headings_at_three_levels() {
        let doc = render("# one\n## two\n### three\n");
        let levels: Vec<u8> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 3]);
    }

    #[test]
    fn link_line_with_label() {
        let doc = render("=> gemini://example.test/  Example capsule\n");
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let InlineSpan::Link {
            url, spans: inner, ..
        } = &spans[0]
        else {
            panic!("expected link, got {:?}", spans[0]);
        };
        assert_eq!(url, "gemini://example.test/");
        assert!(matches!(inner[0], InlineSpan::Text(ref t) if t == "Example capsule"));
    }

    #[test]
    fn link_line_without_label_uses_url_as_display() {
        let doc = render("=> gemini://example.test/page\n");
        let outgoing = doc.outgoing_links();
        assert_eq!(outgoing, vec!["gemini://example.test/page"]);
    }

    #[test]
    fn consecutive_list_items_merge_into_one_list() {
        let doc = render("* one\n* two\n* three\n");
        let Block::List { items, ordered } = &doc.blocks[0] else {
            panic!("expected list");
        };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn consecutive_quote_lines_merge_into_one_quote() {
        let doc = render("> first quoted line\n> second quoted line\n");
        let Block::Quote { blocks } = &doc.blocks[0] else {
            panic!("expected quote");
        };
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn preformatted_block_with_alt_text() {
        let doc = render("```rust\nfn main() {}\n```\n");
        let Block::CodeBlock { language, text } = &doc.blocks[0] else {
            panic!("expected code block");
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() {}\n");
    }

    #[test]
    fn preformatted_swallows_other_prefixes() {
        let doc = render("```\n=> not-a-link\n# not-a-heading\n```\n");
        let Block::CodeBlock { text, .. } = &doc.blocks[0] else {
            panic!("expected code block");
        };
        assert!(text.contains("=> not-a-link"));
        assert!(text.contains("# not-a-heading"));
    }

    #[test]
    fn unprefixed_lines_accumulate_into_paragraph() {
        let doc = render("first line\nsecond line\n\nnext paragraph\n");
        assert_eq!(doc.blocks.len(), 2);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(
            spans
                .iter()
                .filter(|s| matches!(s, InlineSpan::SoftBreak))
                .count(),
            1
        );
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(GemtextEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("gemini:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let doc = registry
            .dispatch(
                &decision,
                &EngineInput::new("gemini://capsule.test/", "# Hello\n\nbody"),
            )
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
    }
}
