/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Statements: control flow (`if` / `while` / `for` / `do`), jumps,
//! local declarations, and the block body each function definition
//! consumes.

use crate::ast::{Block, ForInit, LocalDecl, Stmt, TypeKind};
use crate::error::{Error, ErrorKind};
use crate::token::{Keyword, Punct, TokenKind};

use super::Parser;

/// Discriminator shared by the three single-keyword jumps so they go
/// through one handler.
#[derive(Clone, Copy)]
enum JumpKind {
    Break,
    Continue,
    Discard,
}

impl Parser {
    pub(super) fn block(&mut self) -> Result<Block, Error> {
        let lbrace = self.expect_punct(Punct::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokenKind::Punct(Punct::RBrace)) | None => break,
                _ => stmts.push(self.stmt()?),
            }
        }
        let rbrace = self.expect_punct(Punct::RBrace, "`}`")?;
        Ok(Block {
            stmts,
            span: lbrace.merge(rbrace),
        })
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        match self.peek_kind() {
            Some(TokenKind::Keyword(Keyword::Return)) => return self.parse_return(),
            Some(TokenKind::Keyword(Keyword::If)) => return self.parse_if(),
            Some(TokenKind::Keyword(Keyword::While)) => return self.parse_while(),
            Some(TokenKind::Keyword(Keyword::For)) => return self.parse_for(),
            Some(TokenKind::Keyword(Keyword::Do)) => return self.parse_do(),
            Some(TokenKind::Keyword(Keyword::Break)) => {
                return self.parse_simple_jump(JumpKind::Break);
            },
            Some(TokenKind::Keyword(Keyword::Continue)) => {
                return self.parse_simple_jump(JumpKind::Continue);
            },
            Some(TokenKind::Keyword(Keyword::Discard)) => {
                return self.parse_simple_jump(JumpKind::Discard);
            },
            Some(TokenKind::Keyword(Keyword::Switch)) => return self.parse_switch(),
            Some(TokenKind::Keyword(Keyword::Case)) => return self.parse_case_label(),
            Some(TokenKind::Keyword(Keyword::Default)) => return self.parse_default_label(),
            Some(TokenKind::Punct(Punct::LBrace)) => {
                let b = self.block()?;
                return Ok(Stmt::Block(b));
            },
            _ => {},
        }
        if self.peek_starts_local_decl() {
            return self.local_decl();
        }
        let e = self.expr()?;
        self.expect_punct(Punct::Semi, "`;`")?;
        Ok(Stmt::Expr(e))
    }

    fn parse_return(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        let value = if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::Semi))) {
            None
        } else {
            Some(self.expr()?)
        };
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        Ok(Stmt::Return {
            value,
            span: start.merge(semi),
        })
    }

    fn parse_if(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        self.expect_punct(Punct::LParen, "`(`")?;
        let cond = self.expr()?;
        self.expect_punct(Punct::RParen, "`)`")?;
        let then = Box::new(self.stmt()?);
        let else_ = if matches!(self.peek_kind(), Some(TokenKind::Keyword(Keyword::Else))) {
            self.bump();
            Some(Box::new(self.stmt()?))
        } else {
            None
        };
        let end = match &else_ {
            Some(s) => s.span(),
            None => then.span(),
        };
        Ok(Stmt::If {
            cond,
            then,
            else_,
            span: start.merge(end),
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        self.expect_punct(Punct::LParen, "`(`")?;
        let cond = self.expr()?;
        self.expect_punct(Punct::RParen, "`)`")?;
        let body = Box::new(self.stmt()?);
        let span = start.merge(body.span());
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_do(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        let body = Box::new(self.stmt()?);
        match self.peek_kind() {
            Some(TokenKind::Keyword(Keyword::While)) => {
                self.bump();
            },
            other => {
                return Err(Error::new(
                    ErrorKind::Expected {
                        wanted: "`while` after `do <body>`",
                        got: other.map(|k| k.label()).unwrap_or_else(|| "<eof>".into()),
                    },
                    self.peek_span(),
                ));
            },
        };
        self.expect_punct(Punct::LParen, "`(`")?;
        let cond = self.expr()?;
        self.expect_punct(Punct::RParen, "`)`")?;
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        Ok(Stmt::Do {
            body,
            cond,
            span: start.merge(semi),
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        self.expect_punct(Punct::LParen, "`(`")?;
        let init = if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::Semi))) {
            self.bump();
            ForInit::Empty
        } else if self.peek_starts_local_decl() {
            match self.local_decl()? {
                Stmt::Decl(d) => ForInit::Decl(d),
                _ => unreachable!("local_decl always returns Stmt::Decl"),
            }
        } else {
            let e = self.expr()?;
            self.expect_punct(Punct::Semi, "`;` after for-init")?;
            ForInit::Expr(e)
        };
        let cond = if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::Semi))) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect_punct(Punct::Semi, "`;` after for-cond")?;
        let step = if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::RParen))) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect_punct(Punct::RParen, "`)`")?;
        let body = Box::new(self.stmt()?);
        let span = start.merge(body.span());
        Ok(Stmt::For {
            init,
            cond,
            step,
            body,
            span,
        })
    }

    fn parse_simple_jump(&mut self, kind: JumpKind) -> Result<Stmt, Error> {
        let span_start = self.peek_span();
        self.bump();
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        let span = span_start.merge(semi);
        Ok(match kind {
            JumpKind::Break => Stmt::Break { span },
            JumpKind::Continue => Stmt::Continue { span },
            JumpKind::Discard => Stmt::Discard { span },
        })
    }

    fn peek_starts_local_decl(&self) -> bool {
        match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => {
                if matches!(
                    k,
                    Keyword::Const | Keyword::Lowp | Keyword::Mediump | Keyword::Highp
                ) {
                    return true;
                }
                if TypeKind::from_keyword(*k).is_some() {
                    let next = self.peek_at(1);
                    return !matches!(next, Some(TokenKind::Punct(Punct::LParen)));
                }
                false
            },
            // User-defined struct types: an Ident that names a
            // known struct followed by an Ident (the variable
            // name) starts a local decl.
            Some(TokenKind::Ident(name)) if self.struct_indices.contains_key(name) => {
                let next = self.peek_at(1);
                matches!(next, Some(TokenKind::Ident(_)))
            },
            _ => false,
        }
    }

    fn parse_switch(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        self.expect_punct(Punct::LParen, "`(` after `switch`")?;
        let discriminant = self.expr()?;
        self.expect_punct(Punct::RParen, "`)`")?;
        let body = self.block()?;
        let span = start.merge(body.span);
        Ok(Stmt::Switch {
            discriminant,
            body,
            span,
        })
    }

    fn parse_case_label(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        let value = self.expr()?;
        let colon = self.expect_punct(Punct::Colon, "`:` after `case <value>`")?;
        Ok(Stmt::Case {
            value,
            span: start.merge(colon),
        })
    }

    fn parse_default_label(&mut self) -> Result<Stmt, Error> {
        let start = self.peek_span();
        self.bump();
        let colon = self.expect_punct(Punct::Colon, "`:` after `default`")?;
        Ok(Stmt::Default {
            span: start.merge(colon),
        })
    }

    fn local_decl(&mut self) -> Result<Stmt, Error> {
        let const_start = self.eat_keyword(Keyword::Const);
        let precision = self.maybe_precision_qualifier();
        let ty = self.type_spec()?;
        let (name, name_span) = self.expect_ident("variable name")?;
        let init = if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::Assign))) {
            self.bump();
            Some(self.expr()?)
        } else {
            None
        };
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        let start = const_start.unwrap_or(ty.span);
        Ok(Stmt::Decl(LocalDecl {
            is_const: const_start.is_some(),
            precision,
            ty,
            name,
            name_span,
            init,
            span: start.merge(semi),
        }))
    }
}
