/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Byte-level lexer for ESSL 1.00 source. Produces a flat `Vec<Token>`;
//! parser consumes by index. ESSL source is small (single shader file),
//! so collecting up-front beats streaming.

use crate::error::{Error, ErrorKind};
use crate::span::Span;
use crate::token::{Keyword, Punct, Token, TokenKind};

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        // Whitespace
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
            continue;
        }
        // Comments
        if b == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
        }
        // Identifier or keyword
        if is_ident_start(b) {
            let word_start = i;
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let word = &src[word_start..i];
            let kind = if let Some(k) = Keyword::from_word(word) {
                TokenKind::Keyword(k)
            } else {
                TokenKind::Ident(word.to_string())
            };
            tokens.push(Token { kind, span: Span::new(word_start, i) });
            continue;
        }
        // Number literal
        if b.is_ascii_digit() || (b == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit()) {
            i = lex_number(src, bytes, i, &mut tokens)?;
            continue;
        }
        // Punctuation. Order matters: longer matches first.
        if let Some((p, len)) = match_punct(bytes, i) {
            tokens.push(Token {
                kind: TokenKind::Punct(p),
                span: Span::new(i, i + len),
            });
            i += len;
            continue;
        }
        return Err(Error::new(ErrorKind::UnexpectedByte(b), Span::new(start, start + 1)));
    }
    Ok(tokens)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn lex_number(
    src: &str,
    bytes: &[u8],
    start: usize,
    out: &mut Vec<Token>,
) -> Result<usize, Error> {
    let mut i = start;
    let mut is_float = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        is_float = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        is_float = true;
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return Err(Error::new(ErrorKind::MalformedNumber, Span::new(start, i)));
        }
    }
    // Trailing `f` suffix on floats is ESSL 3.00; tolerate it on the float
    // path. Integer suffixes (`u`, `U`) are 3.00-only and rejected for now.
    if is_float && i < bytes.len() && (bytes[i] == b'f' || bytes[i] == b'F') {
        i += 1;
    }
    let text = &src[start..i];
    let suffix_stripped = text.trim_end_matches(|c| c == 'f' || c == 'F');
    let kind = if is_float {
        let v: f64 = suffix_stripped
            .parse()
            .map_err(|_| Error::new(ErrorKind::MalformedNumber, Span::new(start, i)))?;
        TokenKind::FloatLit(v)
    } else {
        let v: i64 = text
            .parse()
            .map_err(|_| Error::new(ErrorKind::MalformedNumber, Span::new(start, i)))?;
        TokenKind::IntLit(v)
    };
    out.push(Token { kind, span: Span::new(start, i) });
    Ok(i)
}

fn match_punct(bytes: &[u8], i: usize) -> Option<(Punct, usize)> {
    let b = bytes[i];
    let b2 = bytes.get(i + 1).copied();
    // Two-byte matches first.
    if let Some(n) = b2 {
        let pair = (b, n);
        let two = match pair {
            (b'=', b'=') => Some(Punct::Eq),
            (b'!', b'=') => Some(Punct::Ne),
            (b'<', b'=') => Some(Punct::Le),
            (b'>', b'=') => Some(Punct::Ge),
            (b'&', b'&') => Some(Punct::AndAnd),
            (b'|', b'|') => Some(Punct::OrOr),
            (b'+', b'+') => Some(Punct::PlusPlus),
            (b'-', b'-') => Some(Punct::MinusMinus),
            (b'+', b'=') => Some(Punct::PlusEq),
            (b'-', b'=') => Some(Punct::MinusEq),
            (b'*', b'=') => Some(Punct::StarEq),
            (b'/', b'=') => Some(Punct::SlashEq),
            (b'<', b'<') => Some(Punct::Shl),
            (b'>', b'>') => Some(Punct::Shr),
            _ => None,
        };
        if let Some(p) = two {
            return Some((p, 2));
        }
    }
    let one = match b {
        b'(' => Punct::LParen,
        b')' => Punct::RParen,
        b'{' => Punct::LBrace,
        b'}' => Punct::RBrace,
        b'[' => Punct::LBracket,
        b']' => Punct::RBracket,
        b',' => Punct::Comma,
        b';' => Punct::Semi,
        b'.' => Punct::Dot,
        b'=' => Punct::Assign,
        b'+' => Punct::Plus,
        b'-' => Punct::Minus,
        b'*' => Punct::Star,
        b'/' => Punct::Slash,
        b'%' => Punct::Percent,
        b'<' => Punct::Lt,
        b'>' => Punct::Gt,
        b'!' => Punct::Bang,
        b'&' => Punct::Amp,
        b'|' => Punct::Pipe,
        b'^' => Punct::Caret,
        b'~' => Punct::Tilde,
        b'?' => Punct::Question,
        b':' => Punct::Colon,
        _ => return None,
    };
    Some((one, 1))
}
