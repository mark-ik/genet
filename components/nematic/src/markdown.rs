/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Markdown engine.
//!
//! Parses CommonMark via [`pulldown_cmark`] and emits an [`EngineDocument`]
//! made of portable blocks and inline spans. No layout, no rendering — that's
//! `platen`'s job. No network, no I/O — that's the host's job. The engine is
//! a pure transform from markdown bytes to a host-neutral document model.

use inker::{
    Block, DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError,
    EngineInput, InlineSpan, inline_text,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.markdown";

/// CommonMark engine.
pub struct MarkdownEngine;

impl MarkdownEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for MarkdownEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    #[tracing::instrument(
        level = "debug",
        skip(self, input),
        fields(
            address = %input.address,
            body_bytes = input.body.len(),
        ),
    )]
    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let parser = Parser::new(&input.body);
        let mut converter = Converter::default();
        for event in parser {
            converter.handle(event);
        }
        let blocks = converter.finish();
        let title = first_h1_text(&blocks);
        tracing::debug!(
            block_count = blocks.len(),
            has_title = title.is_some(),
            "markdown render complete"
        );
        Ok(EngineDocument {
            address: input.address.clone(),
            title,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| "text/markdown".to_string()),
            // CommonMark has no in-band language declaration. The host
            // supplies a lang during projection if it knows one.
            lang: None,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        })
    }
}

#[derive(Default)]
struct Converter {
    /// Top-level blocks in document order. When a container block (Quote,
    /// List, Item) is open, completed leaf blocks accumulate into the
    /// container's frame instead.
    root_blocks: Vec<Block>,
    /// Stack of currently-open block containers and the children they have
    /// accumulated so far.
    block_stack: Vec<BlockFrame>,
    /// Stack of inline contexts. Each frame is the span list being built for
    /// either a text-accepting block (Heading/Paragraph) or a nested inline
    /// container (Emphasis/Strong/Link). The top frame is where new
    /// text/code/break events append.
    inline_stack: Vec<Vec<InlineSpan>>,
    /// When inside a Link's contents, this records the link metadata waiting
    /// to be combined with the inline frame on End(Link).
    link_stack: Vec<LinkBuilder>,
    /// Depth of any Image we're currently inside; while > 0 we discard URL
    /// and title and only keep the alt text via the inline stack.
    image_depth: u32,
}

struct LinkBuilder {
    url: String,
    title: Option<String>,
}

enum BlockFrame {
    /// Leaf text-accepting block currently collecting inline spans.
    Heading {
        level: u8,
    },
    Paragraph,
    /// Container blocks: hold completed children.
    Quote {
        children: Vec<Block>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Item {
        children: Vec<Block>,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
    },
}

impl Converter {
    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(end) => self.end_tag(end),
            Event::Text(text) => self.push_text(text.into_string()),
            Event::Code(code) => self.push_inline(InlineSpan::Code(code.into_string())),
            Event::SoftBreak => self.push_inline(InlineSpan::SoftBreak),
            Event::HardBreak => self.push_inline(InlineSpan::LineBreak),
            Event::Rule => self.attach_block(Block::Rule),
            // HTML, footnotes, tables, task-list markers, math, definition
            // lists, and metadata blocks are dropped in v1. They emit no
            // text and produce no portable block.
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {},
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.block_stack.push(BlockFrame::Paragraph);
                self.inline_stack.push(Vec::new());
            },
            Tag::Heading { level, .. } => {
                let level = heading_level(level);
                self.block_stack.push(BlockFrame::Heading { level });
                self.inline_stack.push(Vec::new());
            },
            Tag::BlockQuote(_) => {
                self.block_stack.push(BlockFrame::Quote {
                    children: Vec::new(),
                });
            },
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Indented => None,
                    CodeBlockKind::Fenced(info) => {
                        let info = info.into_string();
                        if info.is_empty() { None } else { Some(info) }
                    },
                };
                self.block_stack.push(BlockFrame::CodeBlock {
                    language,
                    text: String::new(),
                });
            },
            Tag::List(start) => {
                self.block_stack.push(BlockFrame::List {
                    ordered: start.is_some(),
                    items: Vec::new(),
                });
            },
            Tag::Item => {
                self.block_stack.push(BlockFrame::Item {
                    children: Vec::new(),
                });
                // Tight-list items emit Text events directly under Item with
                // no enclosing Paragraph tag; open an inline frame so those
                // spans are captured. Loose-list items push their own frame
                // at Tag::Paragraph and ignore this one.
                self.inline_stack.push(Vec::new());
            },
            Tag::Emphasis | Tag::Strong => {
                self.inline_stack.push(Vec::new());
            },
            Tag::Strikethrough | Tag::Superscript | Tag::Subscript => {
                // Strikethrough / superscript / subscript lower to a no-op
                // wrapper for v1: the inline model has no variant for them,
                // but losing the inner text would be worse than losing the
                // styling.
                self.inline_stack.push(Vec::new());
            },
            Tag::Link {
                dest_url, title, ..
            } => {
                self.link_stack.push(LinkBuilder {
                    url: dest_url.into_string(),
                    title: optional(title.into_string()),
                });
                self.inline_stack.push(Vec::new());
            },
            Tag::Image { .. } => {
                // Render images by extracting alt text only (lossy v1). We
                // push an inline frame to collect the alt text, then on
                // End(Image) pop and emit the concatenated text as a plain
                // Text span — preserves the alt's words without committing
                // to an inline image span variant yet.
                self.image_depth = self.image_depth.saturating_add(1);
                self.inline_stack.push(Vec::new());
            },
            // Tables, definition lists, footnotes, HTML blocks, and metadata
            // blocks are dropped in v1. We still push a no-op frame so the
            // matching End event balances.
            Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_) => {
                self.block_stack.push(BlockFrame::Paragraph);
                self.inline_stack.push(Vec::new());
            },
        }
    }

    fn end_tag(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                self.block_stack.pop();
                self.attach_block(Block::Paragraph { spans });
            },
            TagEnd::Heading(_) => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                let level = match self.block_stack.pop() {
                    Some(BlockFrame::Heading { level }) => level,
                    _ => 1,
                };
                self.attach_block(Block::Heading { level, spans });
            },
            TagEnd::BlockQuote(_) => {
                if let Some(BlockFrame::Quote { children }) = self.block_stack.pop() {
                    self.attach_block(Block::Quote { blocks: children });
                }
            },
            TagEnd::CodeBlock => {
                if let Some(BlockFrame::CodeBlock { language, text }) = self.block_stack.pop() {
                    self.attach_block(Block::CodeBlock { language, text });
                }
            },
            TagEnd::List(_) => {
                if let Some(BlockFrame::List { ordered, items }) = self.block_stack.pop() {
                    self.attach_block(Block::List { ordered, items });
                }
            },
            TagEnd::Item => {
                let item_spans = self.inline_stack.pop().unwrap_or_default();
                if let Some(BlockFrame::Item { mut children }) = self.block_stack.pop() {
                    // Capture any spans collected at the Item frame level —
                    // i.e. tight-list items where pulldown-cmark emitted
                    // inline content with no Paragraph tag wrapping it.
                    if !item_spans.is_empty() {
                        children.push(Block::Paragraph { spans: item_spans });
                    }
                    if let Some(BlockFrame::List { items, .. }) = self.block_stack.last_mut() {
                        items.push(children);
                    }
                }
            },
            TagEnd::Emphasis => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                self.push_inline(InlineSpan::Emphasis(spans));
            },
            TagEnd::Strong => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                self.push_inline(InlineSpan::Strong(spans));
            },
            TagEnd::Strikethrough | TagEnd::Superscript | TagEnd::Subscript => {
                // Lower to inner spans (see Start handler).
                let spans = self.inline_stack.pop().unwrap_or_default();
                for span in spans {
                    self.push_inline(span);
                }
            },
            TagEnd::Link => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                if let Some(builder) = self.link_stack.pop() {
                    self.push_inline(InlineSpan::Link {
                        url: builder.url,
                        title: builder.title,
                        spans,
                        predicate: None,
                    });
                }
            },
            TagEnd::Image => {
                let spans = self.inline_stack.pop().unwrap_or_default();
                self.image_depth = self.image_depth.saturating_sub(1);
                let alt = inline_text(&spans);
                if !alt.is_empty() {
                    self.push_inline(InlineSpan::Text(alt));
                }
            },
            TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_) => {
                self.inline_stack.pop();
                self.block_stack.pop();
            },
        }
    }

    fn push_text(&mut self, text: String) {
        // Inside a code block, text events accumulate into the block's
        // verbatim buffer rather than the inline stack.
        if let Some(BlockFrame::CodeBlock { text: buffer, .. }) = self.block_stack.last_mut() {
            buffer.push_str(&text);
            return;
        }
        self.push_inline(InlineSpan::Text(text));
    }

    fn push_inline(&mut self, span: InlineSpan) {
        if let Some(top) = self.inline_stack.last_mut() {
            top.push(span);
            return;
        }
        // No inline frame open — the span has nowhere to attach. This means
        // pulldown-cmark emitted inline content under a block-stack frame
        // we're not handling (a previous occurrence: tight-list items, fixed
        // by opening an inline frame on Tag::Item). Trace it so the next
        // occurrence is loud, not silent.
        tracing::warn!(
            ?span,
            block_stack_depth = self.block_stack.len(),
            "inline span dropped — no open inline frame"
        );
    }

    fn attach_block(&mut self, block: Block) {
        for frame in self.block_stack.iter_mut().rev() {
            match frame {
                BlockFrame::Quote { children } | BlockFrame::Item { children } => {
                    children.push(block);
                    return;
                },
                BlockFrame::Heading { .. }
                | BlockFrame::Paragraph
                | BlockFrame::List { .. }
                | BlockFrame::CodeBlock { .. } => {
                    // These are not container blocks for *other* blocks; keep
                    // walking outward.
                    continue;
                },
            }
        }
        self.root_blocks.push(block);
    }

    fn finish(self) -> Vec<Block> {
        self.root_blocks
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn optional(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn first_h1_text(blocks: &[Block]) -> Option<String> {
    blocks.iter().find_map(|block| match block {
        Block::Heading { level: 1, spans } => {
            let text = inline_text(spans);
            if text.is_empty() { None } else { Some(text) }
        },
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        MarkdownEngine::new()
            .render(&EngineInput::new("md:test", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(MarkdownEngine::new().engine_id(), "nematic.markdown");
    }

    #[test]
    fn h1_becomes_document_title() {
        let doc = render("# Hello, world\n\nbody text");
        assert_eq!(doc.title.as_deref(), Some("Hello, world"));
        assert_eq!(doc.content_type, "text/markdown");
    }

    #[test]
    fn paragraph_with_emphasis_and_link() {
        let doc = render("see *the* [docs](https://mere.test/) please");
        let para = doc.blocks.first().expect("one block");
        let Block::Paragraph { spans } = para else {
            panic!("expected paragraph, got {para:?}");
        };
        // see + emphasis(the) + " " + link("docs") + " please"
        assert!(spans.iter().any(|s| matches!(s, InlineSpan::Emphasis(_))));
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, InlineSpan::Link { url, .. } if url == "https://mere.test/"))
        );
    }

    #[test]
    fn outgoing_links_walks_paragraph_and_quote() {
        let doc =
            render("[a](https://a.test/)\n\n> [b](https://b.test/) and [c](https://c.test/)\n");
        assert_eq!(
            doc.outgoing_links(),
            vec!["https://a.test/", "https://b.test/", "https://c.test/"]
        );
    }

    #[test]
    fn fenced_code_block_preserves_language_and_text() {
        let doc = render("```rust\nfn main() {}\n```\n");
        let Block::CodeBlock { language, text } = &doc.blocks[0] else {
            panic!("expected code block, got {:?}", doc.blocks[0]);
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() {}\n");
    }

    #[test]
    fn unordered_list_with_two_items() {
        let doc = render("- one\n- two\n");
        let Block::List { ordered, items } = &doc.blocks[0] else {
            panic!("expected list, got {:?}", doc.blocks[0]);
        };
        assert!(!ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn ordered_list_marks_ordered() {
        let doc = render("1. first\n2. second\n");
        let Block::List { ordered, items } = &doc.blocks[0] else {
            panic!("expected list, got {:?}", doc.blocks[0]);
        };
        assert!(ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn tight_list_items_carry_paragraph_content() {
        // pulldown-cmark emits Text events directly under Item (no enclosing
        // Tag::Paragraph) for tight lists; the converter must capture them.
        let doc = render("- one\n- two with `code`\n");
        let Block::List { items, .. } = &doc.blocks[0] else {
            panic!("expected list, got {:?}", doc.blocks[0]);
        };
        let Block::Paragraph { spans } = &items[0][0] else {
            panic!(
                "expected first item to wrap a Paragraph, got {:?}",
                items[0]
            );
        };
        assert_eq!(inline_text(spans), "one");
        let Block::Paragraph { spans } = &items[1][0] else {
            panic!(
                "expected second item to wrap a Paragraph, got {:?}",
                items[1]
            );
        };
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, InlineSpan::Code(c) if c == "code"))
        );
    }

    #[test]
    fn loose_list_items_preserve_paragraph_blocks() {
        let doc = render("- one\n\n- two\n");
        let Block::List { items, .. } = &doc.blocks[0] else {
            panic!("expected list, got {:?}", doc.blocks[0]);
        };
        assert_eq!(items.len(), 2);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.len(), 1, "item {i} should carry exactly one block");
            assert!(
                matches!(item[0], Block::Paragraph { .. }),
                "item {i} should wrap a Paragraph"
            );
        }
    }

    #[test]
    fn nested_quote_preserves_inner_paragraph() {
        let doc = render("> outer\n>\n> > nested\n");
        let Block::Quote { blocks } = &doc.blocks[0] else {
            panic!("expected quote, got {:?}", doc.blocks[0]);
        };
        assert!(blocks.iter().any(|b| matches!(b, Block::Quote { .. })));
    }

    #[test]
    fn rule_emits_block() {
        let doc = render("hello\n\n---\n\nworld\n");
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Rule)));
    }

    #[test]
    fn image_alt_text_preserved_as_plain_text() {
        let doc = render("see ![an example](https://mere.test/x.png) here\n");
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let combined = inline_text(spans);
        assert!(combined.contains("an example"));
        assert!(!combined.contains("https://mere.test/x.png"));
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(MarkdownEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("doc:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let doc = registry
            .dispatch(&decision, &EngineInput::new("md:doc-1", "# Title\n\ntext"))
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("Title"));
    }
}
