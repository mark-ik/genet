/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Host-side reuse-lexers for the knot editor.
//!
//! The portable [`illume`] crate stays lean (jotdown + logos). This crate is
//! the precision layer: `InjectionLexer` implementations that reuse a parser the
//! app already ships, registered as overrides on the portable pack. They are
//! optional precision over the always-present logos floor, so highlighting never
//! depends on one of these parsers being compiled in.
//!
//! Today: Markdown via [`pulldown_cmark`]. To follow, each over its existing host
//! tokenizer: CSS (`cssparser`), HTML (`html5ever`), precise JS (`boa_parser`),
//! Turtle/RDF (`oxttl`). [`full_pack`] returns the portable pack plus the reuse
//! overrides registered here.

use illume::{InjectionLexer, InjectionRegistry, Span, SyntaxKind};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub mod editor;

pub use editor::KnotReadout;

/// The portable pack plus the host reuse-lexers registered in this crate. The host
/// calls this to get the full editor injection registry; mods register on top.
pub fn full_pack() -> InjectionRegistry {
    let mut reg = illume::default_pack();
    for label in ["markdown", "md", "commonmark"] {
        reg.register(label, Box::new(MarkdownLexer));
    }
    reg
}

/// CommonMark / Markdown highlighter, reusing [`pulldown_cmark`]'s offset-iter
/// event stream (the same shape jotdown gives the djot highlighter). Each colored
/// construct emits one span from its `Start` to its `End`.
pub struct MarkdownLexer;

/// The highlight class for a pulldown-cmark start tag, or `None` for tags the
/// editor does not color (paragraphs, lists, tables, …).
fn tag_kind(tag: &Tag) -> Option<SyntaxKind> {
    Some(match tag {
        Tag::Heading { .. } => SyntaxKind::Heading,
        Tag::Emphasis => SyntaxKind::Emphasis,
        Tag::Strong => SyntaxKind::Strong,
        Tag::Strikethrough => SyntaxKind::Strikethrough,
        Tag::Link { .. } => SyntaxKind::Link,
        Tag::Image { .. } => SyntaxKind::Image,
        Tag::CodeBlock(_) => SyntaxKind::CodeBlock,
        Tag::BlockQuote(_) => SyntaxKind::Blockquote,
        _ => return None,
    })
}

/// Whether an end tag closes a tracked construct (the symmetric counterpart of
/// [`tag_kind`]; `TagEnd` is a lighter enum than `Tag`).
fn tag_end_tracked(end: &TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Heading(_)
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::CodeBlock
            | TagEnd::BlockQuote(_)
    )
}

impl InjectionLexer for MarkdownLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut stack: Vec<(SyntaxKind, usize)> = Vec::new();
        for (event, range) in Parser::new_ext(inner, Options::all()).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if let Some(kind) = tag_kind(&tag) {
                        stack.push((kind, range.start));
                    }
                },
                Event::End(end) => {
                    if tag_end_tracked(&end) {
                        if let Some((kind, start)) = stack.pop() {
                            spans.push(Span {
                                range: start..range.end,
                                kind,
                            });
                        }
                    }
                },
                // Inline code spans are atomic (no Start/End pair).
                Event::Code(_) => spans.push(Span {
                    range,
                    kind: SyntaxKind::Verbatim,
                }),
                _ => {},
            }
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_lexer_colors_structure() {
        let src = "# H\n\n*em* **st** `code`\n\n> quote\n";
        let kinds: Vec<_> = MarkdownLexer.lex(src).into_iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SyntaxKind::Heading), "{kinds:?}");
        assert!(kinds.contains(&SyntaxKind::Emphasis), "{kinds:?}");
        assert!(kinds.contains(&SyntaxKind::Strong), "{kinds:?}");
        assert!(kinds.contains(&SyntaxKind::Verbatim), "{kinds:?}");
        assert!(kinds.contains(&SyntaxKind::Blockquote), "{kinds:?}");
    }

    #[test]
    fn full_pack_is_the_portable_pack_plus_reuse() {
        let reg = full_pack();
        // Portable pack still present.
        assert!(reg.has("json"));
        assert!(reg.has("rust"));
        assert!(reg.has("lua"));
        // Reuse override added here.
        assert!(reg.has("markdown"));
        assert!(reg.has("md"));
    }
}
