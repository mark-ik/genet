//! CSS and HTML floor lexers.
//!
//! Web table-stakes. These are `logos` floors rather than reuse-lexers over
//! cssparser / html5ever: those crates are parse-oriented (the CSS cascade, the
//! HTML tree) and do not hand back clean highlight byte-spans, so a coarse DFA
//! lexer is both simpler and the right tool, and it stays engine-independent. A
//! host may still override with a precise reuse-lexer later; the floor is always
//! present.

use logos::Logos;

use crate::highlight::{Span, SyntaxKind};
use crate::injection::InjectionLexer;

// --- CSS ----------------------------------------------------------------------

#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum CssToken {
    #[token("/*", css_comment)]
    Comment,
    #[regex(r#""([^"\\]|\\.)*""#)]
    #[regex(r"'([^'\\]|\\.)*'")]
    Str,
    #[regex(r"#[0-9a-fA-F]{3,8}")]
    HexColor,
    #[regex(r"@[A-Za-z-]+")]
    AtRule,
    // Digits in the DFA; the callback extends over a trailing unit (px, em, %), so
    // `10px` is one number with no regex ambiguity against idents.
    #[regex(r"-?[0-9]+(\.[0-9]+)?", css_number)]
    Number,
    #[regex(r"[A-Za-z_][A-Za-z0-9_-]*")]
    Ident,
    // Single char so `/*` (comment) wins over a `/` punctuation run.
    #[regex(r"[{}:;,()>+~*\[\].#=!/]")]
    Punct,
}

fn css_comment<'s>(lex: &mut logos::Lexer<'s, CssToken>) {
    let rest = lex.remainder();
    let len = rest.find("*/").map(|i| i + 2).unwrap_or(rest.len());
    lex.bump(len);
}

/// Extend a matched number over a trailing unit (`px`, `em`, `%`, …).
fn css_number<'s>(lex: &mut logos::Lexer<'s, CssToken>) {
    let rest = lex.remainder();
    let unit = rest
        .find(|c: char| !(c.is_ascii_alphabetic() || c == '%'))
        .unwrap_or(rest.len());
    lex.bump(unit);
}

/// CSS highlighter: `/* */` comments, strings, hex colors, numbers with units,
/// `@`-rules, and structural punctuation. Selector and property names stay plain
/// (distinguishing them needs the cascade, which highlighting does not).
pub struct CssLexer;

impl InjectionLexer for CssLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = CssToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(CssToken::Comment) => SyntaxKind::Comment,
                Ok(CssToken::Str) => SyntaxKind::StringLit,
                Ok(CssToken::HexColor) | Ok(CssToken::Number) => SyntaxKind::Number,
                Ok(CssToken::AtRule) => SyntaxKind::Keyword,
                Ok(CssToken::Punct) => SyntaxKind::Punctuation,
                Ok(CssToken::Ident) | Err(_) => continue,
            };
            spans.push(Span {
                range: lex.span(),
                kind,
            });
        }
        spans
    }
}

// --- HTML ---------------------------------------------------------------------

#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum HtmlToken {
    #[token("<!--", html_comment)]
    Comment,
    // A start or end tag opener with its name: `<div`, `</div`.
    #[regex(r"</?[A-Za-z][A-Za-z0-9:-]*")]
    TagName,
    #[regex(r#""[^"]*""#)]
    #[regex(r"'[^']*'")]
    Str,
    #[regex(r"&[A-Za-z0-9#]+;")]
    Entity,
    // Attribute names and text words (left plain).
    #[regex(r"[A-Za-z_:][A-Za-z0-9_:.-]*")]
    Ident,
    #[token("<")]
    #[token(">")]
    #[token("/>")]
    #[token("=")]
    Punct,
}

fn html_comment<'s>(lex: &mut logos::Lexer<'s, HtmlToken>) {
    let rest = lex.remainder();
    let len = rest.find("-->").map(|i| i + 3).unwrap_or(rest.len());
    lex.bump(len);
}

/// HTML highlighter: `<!-- -->` comments, tag openers (with name), attribute
/// strings, and `&entities;`. Attribute names and text stay plain. Coarse: it
/// colors the source legibly without modeling the full grammar.
pub struct HtmlLexer;

impl InjectionLexer for HtmlLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = HtmlToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(HtmlToken::Comment) => SyntaxKind::Comment,
                Ok(HtmlToken::TagName) => SyntaxKind::Keyword,
                Ok(HtmlToken::Str) => SyntaxKind::StringLit,
                Ok(HtmlToken::Entity) => SyntaxKind::Keyword,
                Ok(HtmlToken::Punct) => SyntaxKind::Punctuation,
                Ok(HtmlToken::Ident) | Err(_) => continue,
            };
            spans.push(Span {
                range: lex.span(),
                kind,
            });
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_colors_comments_strings_numbers_colors() {
        let src = ".btn { color: #fff; /* c */ width: 10px; content: \"x\"; }";
        let spans = CssLexer.lex(src);
        let has = |k: SyntaxKind, t: &str| {
            spans
                .iter()
                .any(|s| s.kind == k && &src[s.range.clone()] == t)
        };
        assert!(has(SyntaxKind::Comment, "/* c */"), "{spans:?}");
        assert!(has(SyntaxKind::Number, "#fff"), "{spans:?}");
        assert!(has(SyntaxKind::Number, "10px"), "{spans:?}");
        assert!(has(SyntaxKind::StringLit, "\"x\""), "{spans:?}");
    }

    #[test]
    fn html_colors_tags_attrs_comments_entities() {
        let src = "<!-- c --><a href=\"x\">hi &amp;</a>";
        let spans = HtmlLexer.lex(src);
        let has = |k: SyntaxKind, t: &str| {
            spans
                .iter()
                .any(|s| s.kind == k && &src[s.range.clone()] == t)
        };
        assert!(has(SyntaxKind::Comment, "<!-- c -->"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "<a"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "</a"), "{spans:?}");
        assert!(has(SyntaxKind::StringLit, "\"x\""), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "&amp;"), "{spans:?}");
    }
}
