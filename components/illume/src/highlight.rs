//! Djot syntax highlighting for the edit surface.
//!
//! Walks jotdown's byte-offset event stream and emits `(range, kind)` spans —
//! the style channel the edit surface paints over the source text. This is the
//! Phase-2 highlighter: zero new dependency beyond jotdown (which the meaning
//! pipe already uses), browser-clean by construction. Inner-language injection of
//! polyglot blocks (the `InjectionLexer` registry) layers on top and is handled
//! separately; here a code / raw block is colored as one region.

use std::ops::Range;

use jotdown::{Container, Event, Parser};

use crate::injection::InjectionRegistry;

/// A highlight class: the vocabulary of the style channel the edit surface paints.
/// Coarse on purpose — token classification, not a parse tree (jotdown owns the
/// document structure). Inner-language injection of a [`SyntaxKind::CodeBlock`] or
/// [`SyntaxKind::RawBlock`] body refines it later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxKind {
    /// A heading line (`#` … `######`).
    Heading,
    /// Emphasis (`_…_`).
    Emphasis,
    /// Strong (`*…*`).
    Strong,
    /// Strikethrough / delete (`{-…-}`).
    Strikethrough,
    /// Highlight / mark (`{=…=}`).
    Mark,
    /// Inline verbatim / code (`` `…` ``).
    Verbatim,
    /// Inline or display math (`$…$`, `$$…$$`).
    Math,
    /// A fenced code block (the whole region, fence included). The injection
    /// registry colors its inner language on top.
    CodeBlock,
    /// A raw block / inline (`` ```=html ``, `` `…`{=html} ``) handed verbatim to
    /// another format. The injection registry colors its inner language on top.
    RawBlock,
    /// A link (`[text](url)` / autolink) — the whole construct.
    Link,
    /// An image (`![alt](url)`).
    Image,
    /// A block quote (`> …`).
    Blockquote,
    /// A fenced div (`::: class` … `:::`).
    Div,

    // --- generic code-token classes, emitted by inner-language injection lexers
    // (the `InjectionLexer` registry); the djot highlighter never emits these ---
    /// A language keyword.
    Keyword,
    /// A string or character literal.
    StringLit,
    /// A numeric literal.
    Number,
    /// A comment.
    Comment,
    /// A function or method name.
    Function,
    /// A type, class, or tag name.
    Type,
    /// An operator or structural punctuation.
    Punctuation,
    /// A bare identifier / variable (often left unstyled).
    Identifier,

    // --- inline prose entities, emitted by the entity pass ([`crate::entity`])
    // over any text (prose, the omnibar, comms), not by the djot highlighter ---
    /// A URL (`https://…`).
    Url,
    /// An `@mention`.
    Mention,
    /// A `#tag`.
    Tag,
    /// An email address.
    Email,
}

/// A source byte range tagged with its highlight class. Ranges are byte offsets
/// into the source string and may nest (a [`SyntaxKind::Strong`] span inside a
/// [`SyntaxKind::Emphasis`] span); the edit surface layers overlapping spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Byte range in the source string.
    pub range: Range<usize>,
    /// The highlight class to paint over `range`.
    pub kind: SyntaxKind,
}

/// The highlight class for a jotdown container, or `None` for containers the
/// editor does not color directly (paragraphs, list structure, sections, table
/// scaffolding, …). Start and End carry the same container, so this is symmetric.
fn kind_of(container: &Container) -> Option<SyntaxKind> {
    Some(match container {
        Container::Heading { .. } => SyntaxKind::Heading,
        Container::Emphasis => SyntaxKind::Emphasis,
        Container::Strong => SyntaxKind::Strong,
        Container::Delete => SyntaxKind::Strikethrough,
        Container::Mark => SyntaxKind::Mark,
        Container::Verbatim => SyntaxKind::Verbatim,
        Container::Math { .. } => SyntaxKind::Math,
        Container::CodeBlock { .. } => SyntaxKind::CodeBlock,
        Container::RawBlock { .. } => SyntaxKind::RawBlock,
        Container::RawInline { .. } => SyntaxKind::RawBlock,
        Container::Link(..) => SyntaxKind::Link,
        Container::Image(..) => SyntaxKind::Image,
        Container::Blockquote => SyntaxKind::Blockquote,
        Container::Div { .. } => SyntaxKind::Div,
        _ => return None,
    })
}

/// Highlight djot source text into `(range, kind)` spans.
///
/// Walks jotdown's `into_offset_iter()` byte-offset event stream. Each colored
/// container emits one span covering its full source extent (the opening
/// delimiter through the closing), built by pairing the `Start` event's range
/// start with the matching `End` event's range end. jotdown guarantees
/// well-nested events, so the open container being closed is always the stack
/// top. Spans may nest; the edit surface layers them.
///
/// The source text is the single source of truth; this reads it and never mutates
/// it. Re-run on edit (cheap at note size).
pub fn highlight_djot(src: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut stack: Vec<(SyntaxKind, usize)> = Vec::new();
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(container, _attrs) => {
                if let Some(kind) = kind_of(&container) {
                    stack.push((kind, range.start));
                }
            }
            Event::End(container) => {
                if kind_of(&container).is_some() {
                    if let Some((kind, start)) = stack.pop() {
                        spans.push(Span {
                            range: start..range.end,
                            kind,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    spans
}

/// The inner byte range and language label of the code/raw block currently open.
struct CodeCtx {
    lang: String,
    inner: Option<(usize, usize)>,
}

impl CodeCtx {
    fn new(lang: &str) -> Self {
        Self {
            lang: lang.to_string(),
            inner: None,
        }
    }

    /// Grow the captured inner range to cover `r` (a `Str` event inside the block).
    fn extend(&mut self, r: Range<usize>) {
        self.inner = Some(match self.inner {
            Some((s, e)) => (s.min(r.start), e.max(r.end)),
            None => (r.start, r.end),
        });
    }
}

/// Like [`highlight_djot`], plus inner-language injection. A code or raw block
/// whose language label has a lexer in `registry` gets its body colored in that
/// language, the injected spans merged on top of the block's region span. A block
/// with no language, or a label with no registered lexer, stays a single
/// [`SyntaxKind::CodeBlock`] / [`SyntaxKind::RawBlock`] region (it renders plain).
///
/// Code blocks do not nest, so one in-flight [`CodeCtx`] suffices. The inner range
/// is the union of the block's `Str` event ranges (the code body between the
/// fences); the registry offsets the inner lexer's spans back into the document.
pub fn highlight(src: &str, registry: &InjectionRegistry) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut stack: Vec<(SyntaxKind, usize)> = Vec::new();
    let mut code: Option<CodeCtx> = None;
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(container, _attrs) => {
                match &container {
                    Container::CodeBlock { language } => {
                        code = Some(CodeCtx::new(language.as_ref()))
                    }
                    Container::RawBlock { format } => code = Some(CodeCtx::new(format.as_ref())),
                    _ => {}
                }
                if let Some(kind) = kind_of(&container) {
                    stack.push((kind, range.start));
                }
            }
            Event::Str(_) => {
                if let Some(ctx) = code.as_mut() {
                    ctx.extend(range.clone());
                }
            }
            Event::End(container) => {
                if matches!(
                    &container,
                    Container::CodeBlock { .. } | Container::RawBlock { .. }
                ) {
                    if let Some(ctx) = code.take() {
                        if !ctx.lang.is_empty() {
                            if let Some((s, e)) = ctx.inner {
                                if let Some(injected) = registry.lex_at(&ctx.lang, &src[s..e], s) {
                                    spans.extend(injected);
                                }
                            }
                        }
                    }
                }
                if kind_of(&container).is_some() {
                    if let Some((kind, start)) = stack.pop() {
                        spans.push(Span {
                            range: start..range.end,
                            kind,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Find the first span of `kind` and return the source slice it covers.
    fn slice_of<'a>(src: &'a str, spans: &[Span], kind: SyntaxKind) -> Option<&'a str> {
        spans
            .iter()
            .find(|s| s.kind == kind)
            .map(|s| &src[s.range.clone()])
    }

    /// Diagnostic dump of the spans for a few samples — run with
    /// `cargo test -p knot-editor -- --nocapture dump_spans` to see jotdown's
    /// actual byte ranges. Not an assertion; documents the mapping.
    #[test]
    fn dump_spans() {
        for src in [
            "# A heading",
            "Some _emphasis_ and *strong* and `code` here.",
            "A [link](https://example.com) and ![pic](p.png).",
            "```rust\nfn main() {}\n```",
            "> a quote\n",
            "::: note\nbody\n:::\n",
        ] {
            eprintln!("--- {src:?}");
            for s in highlight_djot(src) {
                eprintln!("  {:?} {:?} = {:?}", s.kind, s.range.clone(), &src[s.range]);
            }
        }
    }

    #[test]
    fn heading_is_colored() {
        let src = "# Title";
        let spans = highlight_djot(src);
        assert!(
            spans.iter().any(|s| s.kind == SyntaxKind::Heading),
            "expected a Heading span, got {spans:?}"
        );
    }

    #[test]
    fn emphasis_and_strong_cover_their_delimiters() {
        let src = "_em_ *st*";
        let spans = highlight_djot(src);
        assert_eq!(slice_of(src, &spans, SyntaxKind::Emphasis), Some("_em_"));
        assert_eq!(slice_of(src, &spans, SyntaxKind::Strong), Some("*st*"));
    }

    #[test]
    fn inline_code_is_verbatim() {
        let src = "before `code` after";
        let spans = highlight_djot(src);
        assert_eq!(slice_of(src, &spans, SyntaxKind::Verbatim), Some("`code`"));
    }

    #[test]
    fn fenced_code_block_is_one_region() {
        let src = "```rust\nfn main() {}\n```";
        let spans = highlight_djot(src);
        let n = spans
            .iter()
            .filter(|s| s.kind == SyntaxKind::CodeBlock)
            .count();
        assert_eq!(n, 1, "expected one CodeBlock span, got {spans:?}");
    }

    #[test]
    fn nested_spans_both_emit() {
        // Strong nested inside emphasis: both spans present, emphasis enclosing.
        let src = "_a *b* c_";
        let spans = highlight_djot(src);
        let em = spans
            .iter()
            .find(|s| s.kind == SyntaxKind::Emphasis)
            .unwrap();
        let st = spans.iter().find(|s| s.kind == SyntaxKind::Strong).unwrap();
        assert!(
            em.range.start <= st.range.start && st.range.end <= em.range.end,
            "strong {:?} should nest inside emphasis {:?}",
            st.range,
            em.range
        );
    }

    #[test]
    fn plain_text_has_no_spans() {
        assert!(highlight_djot("just plain text, nothing fancy").is_empty());
    }

    #[test]
    fn code_block_injects_inner_language() {
        let reg = crate::pack::default_pack();
        let src = "```json\n{\"a\": 1}\n```";
        let spans = highlight(src, &reg);
        // The whole-block region is still present.
        assert!(spans.iter().any(|s| s.kind == SyntaxKind::CodeBlock));
        // The JSON body is colored: a string token "a" and a number 1.
        let string_slices: Vec<_> = spans
            .iter()
            .filter(|s| s.kind == SyntaxKind::StringLit)
            .map(|s| &src[s.range.clone()])
            .collect();
        assert!(
            string_slices.contains(&"\"a\""),
            "expected a JSON string token, got {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.kind == SyntaxKind::Number && &src[s.range.clone()] == "1"),
            "expected a JSON number token, got {spans:?}"
        );
    }

    #[test]
    fn unregistered_code_language_stays_a_plain_region() {
        let reg = InjectionRegistry::new(); // empty
        let src = "```rust\nfn x() {}\n```";
        let spans = highlight(src, &reg);
        assert!(spans.iter().any(|s| s.kind == SyntaxKind::CodeBlock));
        assert!(
            !spans.iter().any(|s| s.kind == SyntaxKind::Keyword),
            "no injection without a registered lexer"
        );
    }
}
