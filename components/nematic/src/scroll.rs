// Copyright 2026 Mark AB (markik)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scroll engine — real `text/scroll` lowering, plus body-shape dispatch for
//! the other mimetypes a scroll server can serve.
//!
//! The scrolltext grammar lives with its protocol
//! ([`scroll-protocol`](https://crates.io/crates/scroll-protocol), re-exported
//! at [`errand::parse::scrolltext`]); nematic owns only the lowering into the
//! [`Block`] model. Until 2026-08-06 this engine read `text/scroll` as gemtext
//! with a degradation notice, because the spec was unreachable; the spec is
//! now in hand (vendored by michael-lazar/smolnet-portal) and the lowering is
//! real.
//!
//! One mapping is worth naming: scrolltext **link relations** (`[Citation]`,
//! `[+Citation]`, `[-]`…) lower onto [`InlineSpan::Link`]'s open `predicate`
//! field, verbatim as written inside the brackets. That is the seam knot
//! ingestion maps onto kernel `Semantic` edges, so a scroll document's cited
//! sources arrive as semantic statements, not decoration.
//!
//! Dispatch for non-scroll bodies is unchanged: markdown mimetypes go to
//! [`crate::MarkdownEngine`], anything else to [`crate::GemtextEngine`]
//! (scroll servers may serve any mimetype; `text/gemini` is common).

use errand::parse::scrolltext::{self, Polarity, ScrollLine, SpanKind};
use inker::{
    Block, DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput, InlineSpan,
};

use crate::{GemtextEngine, MarkdownEngine};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.scroll";

/// Scroll engine: scrolltext lowering plus inner gemtext / markdown engines
/// for the other body types.
pub struct ScrollEngine {
    gemtext: GemtextEngine,
    markdown: MarkdownEngine,
}

impl ScrollEngine {
    pub fn new() -> Self {
        Self {
            gemtext: GemtextEngine::new(),
            markdown: MarkdownEngine::new(),
        }
    }
}

impl Default for ScrollEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for ScrollEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let content_type = input.content_type.as_deref().map(primary_type);

        if content_type.as_deref().is_some_and(is_scroll_markup) {
            return Ok(self.render_scrolltext(input));
        }

        let inner: &dyn Engine = match content_type.as_deref() {
            Some("text/markdown") | Some("text/x-markdown") => &self.markdown,
            _ => &self.gemtext,
        };
        let mut doc = inner.render(input)?;

        // Override the inner provenance with this engine's own ID so
        // consumers see "nematic.scroll" as the source kind. Inner engine
        // ID is preserved as `source_label` so the dispatch path stays
        // visible.
        let inner_kind = doc.provenance.source_kind.clone();
        doc.provenance = DocumentProvenance {
            source_kind: Some(self.engine_id().to_string()),
            canonical_uri: Some(input.address.clone()),
            fetched_at: None,
            source_label: inner_kind,
        };
        // Trust is the transport's to establish (scroll mandates TLS and
        // permits client certificates); an engine handed a body has nothing
        // to judge, so it says so rather than implying a verdict.
        doc.trust = DocumentTrustState::Unknown;
        Ok(doc)
    }
}

impl ScrollEngine {
    fn render_scrolltext(&self, input: &EngineInput) -> EngineDocument {
        let lines = scrolltext::parse(&input.body);
        let mut lowering = Lowering::default();
        for line in &lines {
            lowering.push(line);
        }
        let (blocks, title, diagnostics) = lowering.finish();

        EngineDocument {
            address: input.address.clone(),
            title,
            content_type: "text/scroll".to_string(),
            // The response's language rides the mimetype parameter host-side;
            // the body itself declares none.
            lang: None,
            provenance: DocumentProvenance {
                source_kind: Some(ENGINE_ID.to_string()),
                canonical_uri: Some(input.address.clone()),
                fetched_at: None,
                source_label: Some("scrolltext".to_string()),
            },
            trust: DocumentTrustState::Unknown,
            diagnostics,
            blocks,
        }
    }
}

fn primary_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
}

fn is_scroll_markup(primary: &str) -> bool {
    matches!(primary, "text/scroll" | "text/x-scroll")
}

// ── The lowering ───────────────────────────────────────────────────────────

#[derive(Default)]
struct Lowering {
    blocks: Vec<Block>,
    title: Option<String>,
    diagnostics: Vec<DocumentDiagnostic>,
    /// Consecutive quote lines, gathered so nesting can be built as a tree.
    quote_run: Vec<(u8, String)>,
    /// Consecutive list items, gathered for the same reason.
    list_run: Vec<(u8, Option<String>, String)>,
}

impl Lowering {
    fn push(&mut self, line: &ScrollLine) {
        // Quote and list runs end at the first line that is not theirs; a
        // blank line separates adjacent runs, exactly as the spec uses it.
        match line {
            ScrollLine::Quote { .. } => {},
            _ => self.flush_quotes(),
        }
        match line {
            ScrollLine::ListItem { .. } => {},
            _ => self.flush_list(),
        }

        match line {
            ScrollLine::Heading { level, text } => {
                if *level == 1 && self.title.is_none() {
                    self.title = Some(text.clone());
                }
                self.blocks.push(Block::Heading {
                    level: *level,
                    spans: inline(text),
                });
            },
            // The spec reads each unprefixed line as its own paragraph:
            // "lines are not reflowed, but they may be word-wrapped".
            ScrollLine::Text(text) => self.blocks.push(Block::Paragraph {
                spans: inline(text),
            }),
            ScrollLine::Blank => {},
            ScrollLine::Quote { depth, text } => {
                self.quote_run.push((*depth, text.clone()));
            },
            ScrollLine::ListItem {
                depth,
                ordinal,
                text,
            } => {
                self.list_run.push((*depth, ordinal.clone(), text.clone()));
            },
            ScrollLine::Link {
                url,
                label,
                relation,
            } => {
                let text = if label.is_empty() { url } else { label };
                self.blocks.push(Block::Paragraph {
                    spans: vec![InlineSpan::Link {
                        url: url.clone(),
                        title: None,
                        spans: vec![InlineSpan::Text(text.clone())],
                        // The relation, verbatim as written inside the
                        // brackets: "Citation", "+Citation", "-", …. Knot
                        // ingestion lifts this onto a Semantic edge.
                        predicate: relation.as_ref().map(relation_predicate),
                    }],
                });
            },
            ScrollLine::InputLink { url, prompt } => {
                // The block model has no input affordance; the link survives
                // and the loss is reported rather than hidden.
                self.blocks.push(Block::Paragraph {
                    spans: vec![InlineSpan::Link {
                        url: url.clone(),
                        title: None,
                        spans: vec![InlineSpan::Text(prompt.clone())],
                        predicate: None,
                    }],
                });
                self.diagnostics.push(DocumentDiagnostic::DegradedRendering(
                    "scroll input link (=:) lowered to a plain link: the block model has no \
                     input affordance"
                        .to_string(),
                ));
            },
            ScrollLine::ThematicBreak => self.blocks.push(Block::Rule),
            ScrollLine::CodeBlock { tag, lines } => self.blocks.push(Block::CodeBlock {
                language: tag.clone(),
                text: lines.join("\n"),
            }),
        }
    }

    fn finish(mut self) -> (Vec<Block>, Option<String>, Vec<DocumentDiagnostic>) {
        self.flush_quotes();
        self.flush_list();
        (self.blocks, self.title, self.diagnostics)
    }

    fn flush_quotes(&mut self) {
        let run = std::mem::take(&mut self.quote_run);
        if !run.is_empty() {
            self.blocks.push(Block::Quote {
                blocks: nest_quotes(&run, 1),
            });
        }
    }

    fn flush_list(&mut self) {
        let run = std::mem::take(&mut self.list_run);
        if !run.is_empty() {
            self.blocks.push(nest_list(&run, 1));
        }
    }
}

/// The predicate string for a link relation: exactly what stood inside the
/// brackets, polarity sign included.
fn relation_predicate(relation: &scrolltext::Relation) -> String {
    let sign = match relation.polarity {
        Polarity::Positive => "+",
        Polarity::Negative => "-",
        Polarity::Neutral => "",
    };
    format!("{sign}{}", relation.tag.as_deref().unwrap_or(""))
}

/// Build nested quote blocks from a run of (depth, text) lines. Lines at the
/// current level become paragraphs; deeper stretches become nested quotes.
fn nest_quotes(run: &[(u8, String)], level: u8) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < run.len() {
        if run[index].0 <= level {
            blocks.push(Block::Paragraph {
                spans: inline(&run[index].1),
            });
            index += 1;
        } else {
            let start = index;
            while index < run.len() && run[index].0 > level {
                index += 1;
            }
            blocks.push(Block::Quote {
                blocks: nest_quotes(&run[start..index], level + 1),
            });
        }
    }
    blocks
}

/// Build a (possibly nested) list from a run of (depth, ordinal, text) items.
/// A list is ordered when any item at its level carries a verbatim ordinal;
/// the ordinal is kept in the item text because the spec forbids renumbering.
fn nest_list(run: &[(u8, Option<String>, String)], level: u8) -> Block {
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut ordered = false;
    let mut index = 0;

    while index < run.len() {
        let (depth, ordinal, text) = &run[index];
        if *depth <= level {
            let mut spans = Vec::new();
            if let Some(marker) = ordinal {
                ordered = true;
                spans.push(InlineSpan::Text(format!("{marker} ")));
            }
            spans.extend(inline(text));
            items.push(vec![Block::Paragraph { spans }]);
            index += 1;
        } else {
            let start = index;
            while index < run.len() && run[index].0 > level {
                index += 1;
            }
            let nested = nest_list(&run[start..index], level + 1);
            match items.last_mut() {
                // The nested list belongs to the item above it.
                Some(item) => item.push(nested),
                // A run that *starts* deep still parses; the nest becomes its
                // own item rather than being dropped.
                None => items.push(vec![nested]),
            }
        }
    }

    Block::List { ordered, items }
}

/// Map scrolltext inline spans onto inker's.
fn inline(text: &str) -> Vec<InlineSpan> {
    scrolltext::spans(text)
        .into_iter()
        .map(|span| match span.kind {
            SpanKind::Plain => InlineSpan::Text(span.text),
            SpanKind::Strong => InlineSpan::Strong(vec![InlineSpan::Text(span.text)]),
            SpanKind::Emphasis => InlineSpan::Emphasis(vec![InlineSpan::Text(span.text)]),
            SpanKind::Code => InlineSpan::Code(span.text),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_scroll(body: &str) -> EngineDocument {
        ScrollEngine::new()
            .render(
                &EngineInput::new("scroll://t/doc.scroll", body).with_content_type("text/scroll"),
            )
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(ScrollEngine::new().engine_id(), "nematic.scroll");
    }

    #[test]
    fn default_body_treated_as_gemtext() {
        let doc = ScrollEngine::new()
            .render(&EngineInput::new("scroll://t/", "# Hello\n"))
            .expect("render");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert_eq!(
            doc.provenance.source_kind.as_deref(),
            Some("nematic.scroll")
        );
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.gemtext")
        );
    }

    #[test]
    fn markdown_content_type_routes_to_markdown_engine() {
        let doc = ScrollEngine::new()
            .render(
                &EngineInput::new("scroll://t/", "# Hello\n\n*emphasis*\n")
                    .with_content_type("text/markdown"),
            )
            .expect("render");
        assert_eq!(doc.content_type, "text/markdown");
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.markdown")
        );
    }

    #[test]
    fn text_scroll_is_parsed_for_real_with_no_degradation() {
        // The lane that was knowingly degraded until 2026-08-06.
        let doc = render_scroll("# Title\n\nA paragraph.\n");
        assert_eq!(doc.title.as_deref(), Some("Title"));
        assert_eq!(doc.content_type, "text/scroll");
        assert_eq!(doc.provenance.source_label.as_deref(), Some("scrolltext"));
        assert!(
            doc.diagnostics.is_empty(),
            "no degradation for plain scrolltext: {:?}",
            doc.diagnostics
        );
    }

    #[test]
    fn five_heading_levels_survive_where_gemtext_has_three() {
        let doc = render_scroll("# 1\n#### 4\n##### 5\n");
        let levels: Vec<u8> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 4, 5]);
    }

    #[test]
    fn the_specs_nested_list_example_lowers_with_verbatim_ordinals() {
        let doc = render_scroll(
            "* Unordered list item 1\n** 1. Ordered sub-list item 1\n** 2. Ordered sub-list item 2\n* Unordered list item 2\n",
        );
        let Block::List { ordered, items } = &doc.blocks[0] else {
            panic!("expected a list, got {:?}", doc.blocks);
        };
        assert!(!ordered, "the outer list is unordered");
        assert_eq!(items.len(), 2);
        // The first item carries the nested ordered list.
        let Block::List {
            ordered: nested_ordered,
            items: nested,
        } = &items[0][1]
        else {
            panic!("expected a nested list in the first item: {:?}", items[0]);
        };
        assert!(nested_ordered, "the sub-list is ordered");
        let Block::Paragraph { spans } = &nested[0][0] else {
            panic!("expected a paragraph item");
        };
        let InlineSpan::Text(first) = &spans[0] else {
            panic!("expected text");
        };
        assert_eq!(first, "1. ", "the ordinal is verbatim, never renumbered");
    }

    #[test]
    fn nested_quotes_become_nested_quote_blocks() {
        let doc = render_scroll("> outer\n>> inner\n> outer again\n");
        let Block::Quote { blocks } = &doc.blocks[0] else {
            panic!("expected a quote");
        };
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[1], Block::Quote { .. }), "depth two nests");
    }

    #[test]
    fn link_relations_become_predicates_verbatim() {
        let doc = render_scroll(
            "=> scroll://example.net/a.txt Cited [Citation]\n=> scroll://example.net/b.txt Contra [-Citation]\n=> scroll://example.net/c.pdf Backing [+]\n=> gemini://misfin.org Plain\n",
        );
        let predicates: Vec<Option<String>> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph { spans } => match &spans[0] {
                    InlineSpan::Link { predicate, .. } => Some(predicate.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            predicates,
            vec![
                Some("Citation".to_string()),
                Some("-Citation".to_string()),
                Some("+".to_string()),
                None,
            ]
        );
    }

    #[test]
    fn inline_markup_reaches_the_span_model() {
        let doc = render_scroll("a *strong* and `code` line\n");
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected a paragraph");
        };
        assert!(spans.iter().any(|s| matches!(s, InlineSpan::Strong(_))));
        assert!(spans.iter().any(|s| matches!(s, InlineSpan::Code(_))));
    }

    #[test]
    fn code_blocks_keep_their_tag_and_a_break_is_a_rule() {
        let doc = render_scroll("```rust\nfn main() {}\n```\n---\n");
        assert_eq!(
            doc.blocks[0],
            Block::CodeBlock {
                language: Some("rust".into()),
                text: "fn main() {}".into(),
            }
        );
        assert_eq!(doc.blocks[1], Block::Rule);
    }

    #[test]
    fn an_input_link_survives_as_a_link_and_reports_its_loss() {
        let doc = render_scroll("=: scroll://example.net/search Search terms\n");
        assert!(matches!(&doc.blocks[0], Block::Paragraph { spans }
            if matches!(&spans[0], InlineSpan::Link { .. })));
        assert!(doc.diagnostics.iter().any(|d| matches!(
            d,
            DocumentDiagnostic::DegradedRendering(m) if m.contains("input")
        )));
    }

    #[test]
    fn no_diagnostic_claims_anything_about_signatures() {
        // The old engine reported on envelope signatures the protocol does
        // not have. Nothing here may resurrect that claim.
        for (content_type, body) in [
            ("text/scroll", "# Hi\n"),
            ("text/gemini", "# Hi\n"),
            ("text/markdown", "# Hi\n"),
        ] {
            let doc = ScrollEngine::new()
                .render(&EngineInput::new("scroll://t/", body).with_content_type(content_type))
                .expect("render");
            for diagnostic in &doc.diagnostics {
                let text = format!("{diagnostic:?}").to_lowercase();
                assert!(
                    !text.contains("signature"),
                    "{content_type}: {diagnostic:?}"
                );
                assert!(!text.contains("envelope"), "{content_type}: {diagnostic:?}");
            }
        }
    }
}
