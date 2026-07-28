// Copyright 2026 Mark AB (markik)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reader-mode HTML fragment engine.
//!
//! html5ever, through `genet-static-dom`, performs the standards-correct parse.
//! This module then lowers only a passive semantic subset into Inker blocks.
//! Scriptable elements, frames, forms, styling authority, event handlers, and
//! active URL schemes never reach the portable document model.

use genet_static_dom::{StaticDocument, StaticNodeId, StaticNodeKind};
use inker::{
    Block, DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput, InlineSpan, TableAlignment, inline_text,
};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.html-fragment";

/// A pure HTML-to-semantic-block lowering.
pub struct HtmlFragmentEngine;

impl HtmlFragmentEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HtmlFragmentEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for HtmlFragmentEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let dom = StaticDocument::parse(&input.body);
        let root = find_element(&dom, dom.document_node(), "body")
            .or_else(|| dom.document_element())
            .unwrap_or_else(|| dom.document_node());
        let mut converter = Converter::new(&dom);
        let blocks = converter.container_blocks(root);
        let title = find_element(&dom, dom.document_node(), "title")
            .map(|node| normalized_text(&text_content(&dom, node)))
            .filter(|title| !title.is_empty())
            .or_else(|| {
                blocks.iter().find_map(|block| match block {
                    Block::Heading { level: 1, spans } => Some(inline_text(spans)),
                    _ => None,
                })
            });
        let lang = find_element(&dom, dom.document_node(), "html")
            .and_then(|node| attribute(&dom, node, "lang"))
            .map(str::to_string)
            .filter(|lang| !lang.is_empty());
        let diagnostics = (converter.removed > 0).then(|| {
            DocumentDiagnostic::DegradedRendering(format!(
                "removed {} active, styled, or unsafe HTML construct{}",
                converter.removed,
                if converter.removed == 1 { "" } else { "s" },
            ))
        });

        Ok(EngineDocument {
            address: input.address.clone(),
            title,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| "text/html".to_string()),
            lang,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics: diagnostics.into_iter().collect(),
            blocks,
        })
    }
}

struct Converter<'a> {
    dom: &'a StaticDocument,
    removed: usize,
}

impl<'a> Converter<'a> {
    fn new(dom: &'a StaticDocument) -> Self {
        Self { dom, removed: 0 }
    }

    fn container_blocks(&mut self, node: StaticNodeId) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut inline = Vec::new();
        for child in self.dom.node(node).children() {
            if self.is_dropped(*child) {
                self.removed += 1;
                continue;
            }
            self.note_stripped_attributes(*child);
            match element_name(self.dom, *child) {
                Some(name) if is_block_element(name) => {
                    flush_paragraph(&mut inline, &mut blocks);
                    self.push_block(*child, name, &mut blocks);
                },
                _ => self.push_inline(*child, &mut inline),
            }
        }
        flush_paragraph(&mut inline, &mut blocks);
        blocks
    }

    fn push_block(&mut self, node: StaticNodeId, name: &str, out: &mut Vec<Block>) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let spans = self.inline_children(node);
                if !spans.is_empty() {
                    out.push(Block::Heading {
                        level: name[1..].parse().expect("heading level"),
                        spans,
                    });
                }
            },
            "p" | "figcaption" | "dt" | "dd" => {
                let spans = self.inline_children(node);
                if !spans.is_empty() {
                    out.push(Block::Paragraph { spans });
                }
            },
            "blockquote" => {
                let blocks = self.container_blocks(node);
                if !blocks.is_empty() {
                    out.push(Block::Quote { blocks });
                }
            },
            "ul" | "ol" => {
                let ordered = name == "ol";
                let items = self
                    .dom
                    .node(node)
                    .children()
                    .iter()
                    .copied()
                    .filter(|child| element_name(self.dom, *child) == Some("li"))
                    .map(|child| self.container_blocks(child))
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    out.push(Block::List { ordered, items });
                }
            },
            "pre" => {
                let text = text_content(self.dom, node);
                if !text.is_empty() {
                    out.push(Block::Preformatted { text });
                }
            },
            "hr" => out.push(Block::Rule),
            "img" => {
                if let Some(image) = self.image_block(node) {
                    out.push(image);
                }
            },
            "table" => {
                if let Some(table) = self.table(node) {
                    out.push(table);
                }
            },
            _ => out.extend(self.container_blocks(node)),
        }
    }

    fn inline_children(&mut self, node: StaticNodeId) -> Vec<InlineSpan> {
        let mut spans = Vec::new();
        for child in self.dom.node(node).children() {
            self.push_inline(*child, &mut spans);
        }
        trim_spans(&mut spans);
        spans
    }

    fn push_inline(&mut self, node: StaticNodeId, out: &mut Vec<InlineSpan>) {
        match self.dom.node(node).kind() {
            StaticNodeKind::Text(text) => push_text(out, text),
            StaticNodeKind::Element { .. } => {
                if self.is_dropped(node) {
                    self.removed += 1;
                    return;
                }
                self.note_stripped_attributes(node);
                let Some(name) = element_name(self.dom, node) else {
                    return;
                };
                match name {
                    "br" => out.push(InlineSpan::LineBreak),
                    "em" | "i" => {
                        let spans = self.inline_children(node);
                        if !spans.is_empty() {
                            out.push(InlineSpan::Emphasis(spans));
                        }
                    },
                    "strong" | "b" => {
                        let spans = self.inline_children(node);
                        if !spans.is_empty() {
                            out.push(InlineSpan::Strong(spans));
                        }
                    },
                    "code" => {
                        let text = normalized_text(&text_content(self.dom, node));
                        if !text.is_empty() {
                            out.push(InlineSpan::Code(text));
                        }
                    },
                    "a" => {
                        let spans = self.inline_children(node);
                        if let Some(url) = attribute(self.dom, node, "href").and_then(safe_url) {
                            out.push(InlineSpan::Link {
                                url,
                                title: attribute(self.dom, node, "title").map(str::to_string),
                                spans,
                                predicate: None,
                            });
                        } else {
                            if attribute(self.dom, node, "href").is_some() {
                                self.removed += 1;
                            }
                            out.extend(spans);
                        }
                    },
                    "img" => {
                        if let Some(alt) = attribute(self.dom, node, "alt") {
                            push_text(out, alt);
                        }
                    },
                    _ => {
                        for child in self.dom.node(node).children() {
                            self.push_inline(*child, out);
                        }
                    },
                }
            },
            _ => {},
        }
    }

    fn image_block(&mut self, node: StaticNodeId) -> Option<Block> {
        let src = match attribute(self.dom, node, "src").and_then(safe_url) {
            Some(src) => src,
            None => {
                if attribute(self.dom, node, "src").is_some() {
                    self.removed += 1;
                }
                return None;
            },
        };
        Some(Block::Image {
            url: src,
            alt: attribute(self.dom, node, "alt")
                .unwrap_or_default()
                .to_string(),
        })
    }

    fn table(&mut self, node: StaticNodeId) -> Option<Block> {
        let mut table_rows = Vec::new();
        collect_named_descendants(self.dom, node, "tr", &mut table_rows);
        let mut header = Vec::new();
        let mut rows = Vec::new();
        for row in table_rows {
            let mut cells = Vec::new();
            let mut has_header = false;
            for child in self.dom.node(row).children() {
                match element_name(self.dom, *child) {
                    Some("th") => {
                        has_header = true;
                        cells.push(self.inline_children(*child));
                    },
                    Some("td") => cells.push(self.inline_children(*child)),
                    _ => {},
                }
            }
            if cells.is_empty() {
                continue;
            }
            if header.is_empty() && has_header {
                header = cells;
            } else {
                rows.push(cells);
            }
        }
        if header.is_empty() && rows.is_empty() {
            None
        } else {
            let columns = header
                .len()
                .max(rows.iter().map(Vec::len).max().unwrap_or_default());
            Some(Block::Table {
                alignments: vec![TableAlignment::None; columns],
                header,
                rows,
            })
        }
    }

    fn is_dropped(&self, node: StaticNodeId) -> bool {
        element_name(self.dom, node).is_some_and(|name| {
            matches!(
                name,
                "script"
                    | "style"
                    | "iframe"
                    | "frame"
                    | "frameset"
                    | "object"
                    | "embed"
                    | "applet"
                    | "template"
                    | "form"
                    | "input"
                    | "button"
                    | "select"
                    | "option"
                    | "textarea"
                    | "meta"
                    | "link"
                    | "base"
                    | "canvas"
                    | "svg"
                    | "math"
            )
        })
    }

    fn note_stripped_attributes(&mut self, node: StaticNodeId) {
        let StaticNodeKind::Element { attrs, .. } = self.dom.node(node).kind() else {
            return;
        };
        self.removed += attrs
            .iter()
            .filter(|attr| {
                let name = attr.name.local.as_ref();
                name == "style"
                    || name.starts_with("on")
                    || matches!(name, "srcdoc" | "srcset" | "integrity" | "nonce")
            })
            .count();
    }
}

fn element_name(dom: &StaticDocument, node: StaticNodeId) -> Option<&str> {
    match dom.node(node).kind() {
        StaticNodeKind::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn attribute<'a>(dom: &'a StaticDocument, node: StaticNodeId, wanted: &str) -> Option<&'a str> {
    match dom.node(node).kind() {
        StaticNodeKind::Element { attrs, .. } => attrs
            .iter()
            .find(|attr| attr.name.local.as_ref() == wanted)
            .map(|attr| attr.value.as_ref()),
        _ => None,
    }
}

fn find_element(dom: &StaticDocument, node: StaticNodeId, wanted: &str) -> Option<StaticNodeId> {
    if element_name(dom, node) == Some(wanted) {
        return Some(node);
    }
    dom.node(node)
        .children()
        .iter()
        .find_map(|child| find_element(dom, *child, wanted))
}

fn collect_named_descendants(
    dom: &StaticDocument,
    node: StaticNodeId,
    wanted: &str,
    out: &mut Vec<StaticNodeId>,
) {
    for child in dom.node(node).children() {
        if element_name(dom, *child) == Some(wanted) {
            out.push(*child);
        } else {
            collect_named_descendants(dom, *child, wanted, out);
        }
    }
}

fn text_content(dom: &StaticDocument, node: StaticNodeId) -> String {
    let mut text = String::new();
    collect_text(dom, node, &mut text);
    text
}

fn collect_text(dom: &StaticDocument, node: StaticNodeId, out: &mut String) {
    match dom.node(node).kind() {
        StaticNodeKind::Text(text) => out.push_str(text),
        StaticNodeKind::Element { name, .. }
            if matches!(name.local.as_ref(), "script" | "style" | "iframe") => {},
        _ => {
            for child in dom.node(node).children() {
                collect_text(dom, *child, out);
            }
        },
    }
}

fn is_block_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "li"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
            | "img"
    )
}

fn safe_url(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let scheme = value
        .split_once(':')
        .filter(|(prefix, _)| {
            let mut chars = prefix.chars();
            chars
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        })
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    match scheme.as_deref() {
        None
        | Some(
            "http" | "https" | "gemini" | "gopher" | "finger" | "spartan" | "nex" | "guppy"
            | "mailto",
        ) => Some(value.to_string()),
        _ => None,
    }
}

fn push_text(out: &mut Vec<InlineSpan>, raw: &str) {
    let text = collapse_whitespace(raw);
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(InlineSpan::Text(previous)) => previous.push_str(&text),
        _ => out.push(InlineSpan::Text(text)),
    }
}

fn collapse_whitespace(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let starts_with_space = raw.chars().next().is_some_and(char::is_whitespace);
    for ch in raw.chars() {
        if ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            } else if pending_space && starts_with_space {
                out.push(' ');
            }
            out.push(ch);
            pending_space = false;
        }
    }
    if pending_space && !out.is_empty() {
        out.push(' ');
    }
    out
}

fn normalized_text(raw: &str) -> String {
    collapse_whitespace(raw).trim().to_string()
}

fn trim_spans(spans: &mut Vec<InlineSpan>) {
    if let Some(InlineSpan::Text(text)) = spans.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(InlineSpan::Text(text)) = spans.last_mut() {
        *text = text.trim_end().to_string();
    }
    spans.retain(|span| !matches!(span, InlineSpan::Text(text) if text.is_empty()));
}

fn flush_paragraph(inline: &mut Vec<InlineSpan>, blocks: &mut Vec<Block>) {
    trim_spans(inline);
    if !inline.is_empty() {
        blocks.push(Block::Paragraph {
            spans: std::mem::take(inline),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        HtmlFragmentEngine::new()
            .render(
                &EngineInput::new("https://clips.test/article", body)
                    .with_content_type("text/html"),
            )
            .expect("render HTML")
    }

    #[test]
    fn lowers_reader_structure_and_links() {
        let doc = render(
            r#"<article lang="en">
                <h1>Reader <em>view</em></h1>
                <p>A <strong>safe</strong> <a href="/source">link</a>.</p>
                <ul><li>one</li><li>two</li></ul>
                <table><tr><th>Name</th><th>Value</th></tr>
                  <tr><td>A</td><td>42</td></tr></table>
                <img src="https://clips.test/image.png" alt="diagram">
            </article>"#,
        );

        assert_eq!(doc.title.as_deref(), Some("Reader view"));
        assert_eq!(doc.outgoing_links(), vec!["/source"]);
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(
            doc.blocks
                .iter()
                .any(|block| matches!(block, Block::List { items, .. } if items.len() == 2))
        );
        assert!(doc.blocks.iter().any(|block| matches!(block, Block::Table { header, rows, .. } if header.len() == 2 && rows.len() == 1)));
        assert!(
            doc.blocks
                .iter()
                .any(|block| matches!(block, Block::Image { alt, .. } if alt == "diagram"))
        );
    }

    #[test]
    fn strips_active_elements_handlers_styles_and_unsafe_urls() {
        let doc = render(
            r#"<h1 onclick="steal()" style="display:none">Visible</h1>
                <script>SECRET_SCRIPT()</script>
                <style>.steal { background: url(secret) }</style>
                <iframe srcdoc="<p>SECRET_FRAME</p>"></iframe>
                <form><input value="SECRET_FORM"><button>SECRET_BUTTON</button></form>
                <p><a href="javascript:SECRET_LINK()">keep this text</a></p>
                <img src="data:text/html,SECRET_IMAGE" alt="unsafe">"#,
        );
        let flattened = format!("{doc:?}");

        assert!(flattened.contains("Visible"));
        assert!(flattened.contains("keep this text"));
        for forbidden in [
            "SECRET_SCRIPT",
            "SECRET_FRAME",
            "SECRET_FORM",
            "SECRET_BUTTON",
            "SECRET_LINK",
            "SECRET_IMAGE",
            "display:none",
            "background:",
            "onclick",
        ] {
            assert!(
                !flattened.contains(forbidden),
                "{forbidden} survived: {flattened}"
            );
        }
        assert_eq!(doc.outgoing_links(), Vec::<&str>::new());
        assert!(!doc.diagnostics.is_empty());
    }
}
