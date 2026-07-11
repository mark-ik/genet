//! Inner-language injection: the pluggable lexer registry.
//!
//! A polyglot block (a `=html` raw block, a ` ```rhai ` fence, a `{.mere-script
//! lang=…}` div) carries an inner language. Highlighting it is the editor's
//! headline move. The dispatch is one seam — a registry of [`InjectionLexer`]
//! keyed by language label — with three tiers, all the same trait:
//!
//! 1. **Precise hand-lexers** for the core vocabulary (quick-xml for svg/xml,
//!    html5ever for html, rhai's tokenizer for scripts).
//! 2. **A curated `logos` pack** for broad coverage (a DFA lexer, the fastest
//!    pure-Rust option).
//! 3. **Mod lexers** registered at runtime — the "lil mod parser for your
//!    concern" path.
//!
//! A label with no registered lexer renders plain. Injection needs only token
//! coloring, not a parse tree (jotdown owns the outer structure), so a lexer is
//! both the right tool and the fastest.

use std::collections::HashMap;

use crate::highlight::Span;

/// A lexer that colors the inner content of a polyglot block in its own language.
/// Given the block's inner source text, return highlight spans whose byte ranges
/// are **relative to that inner text**; the registry offsets them into the
/// document (see [`InjectionRegistry::lex_at`]).
///
/// `Send + Sync` so a registry can be shared across the host's actors / threads.
pub trait InjectionLexer: Send + Sync {
    /// Lex `inner` (a polyglot block's body) into highlight spans relative to it.
    fn lex(&self, inner: &str) -> Vec<Span>;
}

/// Maps a language label (a fence info string or a `lang=` attribute) to its
/// [`InjectionLexer`]. The one engine seam behind the whole injection story:
/// built-in hand-lexers and the `logos` pack register here, and a mod registers
/// its own lexer the same way. Labels match case-insensitively. A label with no
/// registered lexer yields `None`, so the block renders plain.
#[derive(Default)]
pub struct InjectionRegistry {
    lexers: HashMap<String, Box<dyn InjectionLexer>>,
}

impl InjectionRegistry {
    /// An empty registry. The host populates it with the built-in tiers plus any
    /// mod lexers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `lexer` for `label` (matched case-insensitively). Replaces any
    /// existing lexer for that label, so a mod can override a built-in.
    pub fn register(&mut self, label: impl Into<String>, lexer: Box<dyn InjectionLexer>) {
        self.lexers.insert(label.into().to_ascii_lowercase(), lexer);
    }

    /// Whether a lexer is registered for `label`.
    pub fn has(&self, label: &str) -> bool {
        self.lexers.contains_key(&label.to_ascii_lowercase())
    }

    /// Lex `inner` with the lexer registered for `label`, or `None` if none is
    /// (the block renders plain). Spans are relative to `inner`; use
    /// [`lex_at`](Self::lex_at) to place them in the document.
    pub fn lex(&self, label: &str, inner: &str) -> Option<Vec<Span>> {
        self.lexers
            .get(&label.to_ascii_lowercase())
            .map(|lexer| lexer.lex(inner))
    }

    /// Lex `inner` and offset every span by `base` into the document's coordinate
    /// space, where `base` is the inner text's start byte in the source. `None`
    /// if no lexer is registered for `label`. This is what the highlighter calls
    /// for a code / raw block: it has the block's inner range, lexes it, and
    /// merges the offset spans into the document's `(range, kind)` channel.
    pub fn lex_at(&self, label: &str, inner: &str, base: usize) -> Option<Vec<Span>> {
        self.lex(label, inner).map(|spans| {
            spans
                .into_iter()
                .map(|s| Span {
                    range: (s.range.start + base)..(s.range.end + base),
                    kind: s.kind,
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::SyntaxKind;

    /// A toy lexer that colors the whole input as a comment. Stands in for a real
    /// `logos` / hand lexer to exercise registration, dispatch, and offsetting.
    struct WholeAsComment;
    impl InjectionLexer for WholeAsComment {
        fn lex(&self, inner: &str) -> Vec<Span> {
            vec![Span {
                range: 0..inner.len(),
                kind: SyntaxKind::Comment,
            }]
        }
    }

    #[test]
    fn register_and_dispatch_case_insensitively() {
        let mut reg = InjectionRegistry::new();
        assert!(!reg.has("toy"));
        reg.register("Toy", Box::new(WholeAsComment));
        assert!(reg.has("toy"));
        assert!(reg.has("TOY"));
        let spans = reg.lex("toy", "abcd").unwrap();
        assert_eq!(
            spans,
            vec![Span {
                range: 0..4,
                kind: SyntaxKind::Comment
            }]
        );
    }

    #[test]
    fn unregistered_label_renders_plain() {
        let reg = InjectionRegistry::new();
        assert!(reg.lex("nope", "x").is_none());
        assert!(reg.lex_at("nope", "x", 5).is_none());
    }

    #[test]
    fn lex_at_offsets_spans_into_the_document() {
        let mut reg = InjectionRegistry::new();
        reg.register("toy", Box::new(WholeAsComment));
        let spans = reg.lex_at("toy", "abcd", 10).unwrap();
        assert_eq!(
            spans,
            vec![Span {
                range: 10..14,
                kind: SyntaxKind::Comment
            }]
        );
    }

    #[test]
    fn a_mod_overrides_a_builtin() {
        struct WholeAsString;
        impl InjectionLexer for WholeAsString {
            fn lex(&self, inner: &str) -> Vec<Span> {
                vec![Span {
                    range: 0..inner.len(),
                    kind: SyntaxKind::StringLit,
                }]
            }
        }
        let mut reg = InjectionRegistry::new();
        reg.register("x", Box::new(WholeAsComment));
        reg.register("x", Box::new(WholeAsString));
        assert_eq!(reg.lex("x", "ab").unwrap()[0].kind, SyntaxKind::StringLit);
    }
}
