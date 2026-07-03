/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Parser entry point + shared token-stream helpers. Grammar handlers
//! live in three submodules so each stays inspectable in one screen:
//!
//! - [`decl`] — external declarations (precision, globals, functions).
//! - [`stmt`] — statements (control flow, jumps, locals, blocks).
//! - [`expr`] — Pratt-driven expressions and primary atoms.
//!
//! Each submodule adds methods to [`Parser`] via `impl Parser { ... }`
//! blocks, so the call graph stays a single state machine despite the
//! split. The token-stream helpers ([`Parser::peek`] / [`Parser::bump`]
//! / [`Parser::expect_punct`] / [`Parser::expect_ident`] /
//! [`Parser::eat_keyword`]) live here; submodules read them through
//! private-but-child-visible Rust scoping rules.

use crate::ast::TranslationUnit;
use crate::error::{Error, ErrorKind};
use crate::span::Span;
use crate::token::{Keyword, Punct, Token, TokenKind};

mod decl;
mod expr;
mod stmt;

pub fn parse(tokens: Vec<Token>, src_len: usize) -> Result<TranslationUnit, Error> {
    let mut p = Parser {
        tokens,
        idx: 0,
        src_len,
        struct_indices: std::collections::HashMap::new(),
    };
    let start = p.peek_span().start;
    let mut decls = Vec::new();
    while !p.at_end() {
        decls.push(p.external_decl()?);
    }
    Ok(TranslationUnit {
        decls,
        span: Span::new(start, src_len),
        // Filled by the upper-layer [`crate::parse_source`] when the
        // raw text carried a `#version` directive; the byte-level
        // parser itself does not see that directive.
        version: None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    idx: usize,
    src_len: usize,
    /// User-defined struct names seen so far at file scope,
    /// mapped to their index in the translation unit's struct
    /// registry (assigned in source order). `type_spec` accepts
    /// any identifier in this map as a user-named struct type.
    struct_indices: std::collections::HashMap<String, u32>,
}

impl Parser {
    fn at_end(&self) -> bool {
        self.idx >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.idx)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek().map(|t| &t.kind)
    }

    fn peek_span(&self) -> Span {
        match self.peek() {
            Some(t) => t.span,
            None => Span::new(self.src_len, self.src_len),
        }
    }

    fn bump(&mut self) -> Option<Token> {
        if self.idx < self.tokens.len() {
            let t = self.tokens[self.idx].clone();
            self.idx += 1;
            Some(t)
        } else {
            None
        }
    }

    fn expect_punct(&mut self, want: Punct, label: &'static str) -> Result<Span, Error> {
        match self.peek_kind() {
            Some(TokenKind::Punct(p)) if *p == want => {
                let sp = self.peek_span();
                self.bump();
                Ok(sp)
            },
            Some(k) => Err(Error::new(
                ErrorKind::Expected {
                    wanted: label,
                    got: k.label(),
                },
                self.peek_span(),
            )),
            None => Err(Error::new(
                ErrorKind::UnexpectedEof { wanted: label },
                self.peek_span(),
            )),
        }
    }

    fn expect_ident(&mut self, label: &'static str) -> Result<(String, Span), Error> {
        match self.peek_kind() {
            Some(TokenKind::Ident(_)) => {
                let span = self.peek_span();
                let name = match self.bump().unwrap().kind {
                    TokenKind::Ident(s) => s,
                    _ => unreachable!(),
                };
                Ok((name, span))
            },
            Some(k) => Err(Error::new(
                ErrorKind::Expected {
                    wanted: label,
                    got: k.label(),
                },
                self.peek_span(),
            )),
            None => Err(Error::new(
                ErrorKind::UnexpectedEof { wanted: label },
                self.peek_span(),
            )),
        }
    }

    fn eat_keyword(&mut self, kw: Keyword) -> Option<Span> {
        match self.peek_kind() {
            Some(TokenKind::Keyword(k)) if *k == kw => {
                let sp = self.peek_span();
                self.bump();
                Some(sp)
            },
            _ => None,
        }
    }

    /// Peek the token at `idx + offset`, or `None` if past the end. Used
    /// by the two-token lookaheads in [`stmt::peek_starts_local_decl`]
    /// and [`decl::parse_params`].
    fn peek_at(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens.get(self.idx + offset).map(|t| &t.kind)
    }
}
