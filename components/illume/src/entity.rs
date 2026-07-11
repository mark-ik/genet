//! Inline prose-entity highlighting: URLs, `@mentions`, `#tags`, and emails found
//! amid ordinary text.
//!
//! Distinct from [`highlight`](crate::highlight) (djot document structure) and the
//! injection [`pack`](crate::pack) (whole code languages): the entity pass scans
//! *any* text for embedded entities and emits their spans, so a host can enrich the
//! omnibar, comms, labels, or note prose the same way. A `logos` lexer matches the
//! entity shapes and skips everything between them.

use logos::Logos;

use crate::highlight::{Span, SyntaxKind};

/// The entity shapes matched in prose. Everything between matches is unmatched
/// (`logos` yields `Err`, which [`entities`] drops).
#[derive(Logos, Debug, PartialEq)]
enum Entity {
    /// A URL with an explicit scheme. Bare-domain linkification is deliberately
    /// out (too ambiguous in prose); requiring a scheme keeps it false-positive-free.
    #[regex(r"https?://[^\s<>()\[\]]+", priority = 3)]
    Url,
    /// An email address. Higher priority than [`Entity::Mention`] so `a@b.com`
    /// lexes as one email rather than a stray mention at the `@`.
    #[regex(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}", priority = 3)]
    Email,
    /// An `@mention`: `@` then a name.
    #[regex(r"@[A-Za-z0-9_][A-Za-z0-9_.-]*")]
    Mention,
    /// A `#tag`: `#` then a name (a letter first, so `#1` is not a tag).
    #[regex(r"#[A-Za-z][A-Za-z0-9_/-]*")]
    Tag,
}

impl Entity {
    fn kind(&self) -> SyntaxKind {
        match self {
            Entity::Url => SyntaxKind::Url,
            Entity::Email => SyntaxKind::Email,
            Entity::Mention => SyntaxKind::Mention,
            Entity::Tag => SyntaxKind::Tag,
        }
    }
}

/// Scan `text` for inline entities (URLs, emails, `@mentions`, `#tags`) and return
/// their spans in source order. Non-entity text produces no spans. This is the
/// pass a host runs over the omnibar, comms, or any prose surface; the editor can
/// layer it over the djot highlight.
pub fn entities(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut lex = Entity::lexer(text);
    while let Some(tok) = lex.next() {
        if let Ok(entity) = tok {
            spans.push(Span {
                range: lex.span(),
                kind: entity.kind(),
            });
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(text: &str) -> Vec<(SyntaxKind, &str)> {
        entities(text)
            .into_iter()
            .map(|s| (s.kind, &text[s.range]))
            .collect()
    }

    #[test]
    fn matches_url_mention_tag() {
        let got = pairs("see https://ex.com/p and @ada about #rust");
        assert!(
            got.contains(&(SyntaxKind::Url, "https://ex.com/p")),
            "{got:?}"
        );
        assert!(got.contains(&(SyntaxKind::Mention, "@ada")), "{got:?}");
        assert!(got.contains(&(SyntaxKind::Tag, "#rust")), "{got:?}");
    }

    #[test]
    fn email_beats_mention() {
        let got = pairs("mail ada@example.com now");
        assert!(
            got.iter()
                .any(|(k, s)| *k == SyntaxKind::Email && *s == "ada@example.com"),
            "{got:?}"
        );
        assert!(
            !got.iter().any(|(k, _)| *k == SyntaxKind::Mention),
            "{got:?}"
        );
    }

    #[test]
    fn plain_text_has_no_entities() {
        assert!(entities("just some words, nothing special here").is_empty());
    }

    #[test]
    fn tag_needs_a_letter_first() {
        // `#1` (an issue ref) is not a tag; `#todo` is.
        let t = "item #1 and #todo";
        let got = pairs(t);
        assert_eq!(got, vec![(SyntaxKind::Tag, "#todo")], "{got:?}");
    }
}
