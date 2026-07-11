//! Lua floor lexer (for piccolo's Lua and Lua note blocks).
//!
//! Lua is not C-family: `--` line comments, `--[[ … ]]` block comments, and
//! `[[ … ]]` long strings. piccolo is not a dependency, so this is a `logos`
//! lexer rather than a reuse-lexer. Coarse: long-bracket levels (`[==[ … ]==]`)
//! collapse to the plain `[[ … ]]` form, enough to keep a script legible.

use logos::Logos;

use crate::highlight::{Span, SyntaxKind};
use crate::injection::InjectionLexer;

#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum LuaToken {
    // `--` opens a comment; the callback consumes a `--[[ … ]]` block (when `[[`
    // follows) or the rest of the line. One token, so the block form is not lost to
    // a longer line-comment match.
    #[token("--", lua_comment)]
    Comment,
    // `[[ … ]]` long string.
    #[token("[[", long_bracket)]
    #[regex(r#""([^"\\]|\\.)*""#)]
    #[regex(r"'([^'\\]|\\.)*'")]
    Str,
    #[regex(r"0[xX][0-9a-fA-F_]+")]
    #[regex(r"[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?")]
    Number,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,
    // Single char (not a `+` run), so the `--` and `[[` tokens, which start with
    // characters in this class, win on length over a punctuation run.
    #[regex(r"[+\-*/%^#=<>~(){}\[\];:,.]")]
    Punct,
}

/// Consume a `[[ … ]]` long string from just past the opening `[[` to the closing
/// `]]`, or to end of input if unterminated.
fn long_bracket<'s>(lex: &mut logos::Lexer<'s, LuaToken>) {
    let rest = lex.remainder();
    let len = rest.find("]]").map(|i| i + 2).unwrap_or(rest.len());
    lex.bump(len);
}

/// A `--` comment: a `--[[ … ]]` block when `[[` follows the dashes, else the rest
/// of the line.
fn lua_comment<'s>(lex: &mut logos::Lexer<'s, LuaToken>) {
    let rest = lex.remainder();
    if let Some(after) = rest.strip_prefix("[[") {
        let body = after.find("]]").map(|i| i + 2).unwrap_or(after.len());
        lex.bump(2 + body);
    } else {
        let line = rest.find('\n').unwrap_or(rest.len());
        lex.bump(line);
    }
}

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Lua highlighter (a `logos` DFA).
pub struct LuaLexer;

impl InjectionLexer for LuaLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = LuaToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(LuaToken::Comment) => SyntaxKind::Comment,
                Ok(LuaToken::Str) => SyntaxKind::StringLit,
                Ok(LuaToken::Number) => SyntaxKind::Number,
                Ok(LuaToken::Punct) => SyntaxKind::Punctuation,
                Ok(LuaToken::Ident) if LUA_KEYWORDS.contains(&lex.slice()) => SyntaxKind::Keyword,
                Ok(LuaToken::Ident) | Err(_) => continue,
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
    fn lua_colors_keywords_strings_numbers_comments() {
        let src = "-- c\nlocal x = 42\nfunction f() return \"hi\" end\nlocal s = [[long]]";
        let spans = LuaLexer.lex(src);
        let has = |k: SyntaxKind, t: &str| {
            spans
                .iter()
                .any(|s| s.kind == k && &src[s.range.clone()] == t)
        };
        assert!(has(SyntaxKind::Comment, "-- c"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "local"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "function"), "{spans:?}");
        assert!(has(SyntaxKind::Number, "42"), "{spans:?}");
        assert!(has(SyntaxKind::StringLit, "\"hi\""), "{spans:?}");
        assert!(
            has(SyntaxKind::StringLit, "[[long]]"),
            "long string: {spans:?}"
        );
    }

    #[test]
    fn block_comment_spans_lines() {
        let src = "--[[ a\nb ]] local";
        let spans = LuaLexer.lex(src);
        assert!(
            spans
                .iter()
                .any(|s| s.kind == SyntaxKind::Comment && src[s.range.clone()].contains('\n')),
            "block comment should span lines: {spans:?}"
        );
    }
}
