/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Expressions: Pratt-driven climbing-precedence loop, primary atoms
//! (literals, identifiers, parenthesized expressions, constructor calls),
//! and the binding-power tables that govern operator precedence.
//!
//! Technique borrowed from chumsky's `pratt` module and matklad's
//! "Simple but Powerful Pratt Parsing"
//! (<https://matklad.github.io/2020/04/13/simple-but-powerful-pratt-parsing.html>).
//! Higher binding power = tighter binding. Left-associative pairs have
//! `r_bp > l_bp`; right-associative have `r_bp < l_bp`.

use crate::ast::{AssignOp, BinOp, Expr, TypeKind, UnaryOp};
use crate::error::{Error, ErrorKind};
use crate::span::Span;
use crate::token::{Keyword, Punct, TokenKind};

use super::Parser;

impl Parser {
    pub(super) fn expr(&mut self) -> Result<Expr, Error> {
        self.expr_bp(0)
    }

    fn expr_bp(&mut self, min_bp: u8) -> Result<Expr, Error> {
        // Prefix or primary.
        let mut lhs = if let Some(TokenKind::Punct(p)) = self.peek_kind() {
            if let Some(r_bp) = prefix_bp(*p) {
                let op_punct = *p;
                let op_span = self.peek_span();
                self.bump();
                let rhs = self.expr_bp(r_bp)?;
                let op = prefix_op_from_punct(op_punct);
                let span = op_span.merge(rhs.span());
                Expr::Unary { op, expr: Box::new(rhs), span }
            } else {
                self.primary_expr()?
            }
        } else {
            self.primary_expr()?
        };

        // Postfix + infix loop.
        loop {
            let p = match self.peek_kind() {
                Some(TokenKind::Punct(p)) => *p,
                _ => break,
            };
            if let Some(l_bp) = postfix_bp(p) {
                if l_bp < min_bp {
                    break;
                }
                let op_span = self.peek_span();
                self.bump();
                lhs = self.apply_postfix(lhs, p, op_span)?;
                continue;
            }
            if let Some((l_bp, r_bp)) = infix_bp(p) {
                if l_bp < min_bp {
                    break;
                }
                self.bump();
                let rhs = self.expr_bp(r_bp)?;
                lhs = make_infix(p, lhs, rhs);
                continue;
            }
            break;
        }
        Ok(lhs)
    }

    fn apply_postfix(&mut self, lhs: Expr, p: Punct, op_span: Span) -> Result<Expr, Error> {
        match p {
            Punct::LParen => {
                let args = self.call_args()?;
                let rparen = self.expect_punct(Punct::RParen, "`)`")?;
                // Callable shape is `ident(args)`; non-ident bases reach
                // here as `<computed>` so the validator rejects them.
                let callee_name = match &lhs {
                    Expr::Ident { name, .. } => name.clone(),
                    _ => "<computed>".to_string(),
                };
                let lhs_span = lhs.span();
                Ok(Expr::Call {
                    callee: callee_name,
                    callee_span: lhs_span,
                    args,
                    span: lhs_span.merge(rparen),
                })
            },
            Punct::Dot => {
                let (field, field_span) = self.expect_ident("field or swizzle")?;
                let base_span = lhs.span();
                Ok(Expr::Member {
                    base: Box::new(lhs),
                    field,
                    field_span,
                    span: base_span.merge(field_span),
                })
            },
            Punct::LBracket => {
                let index = self.expr()?;
                let rbr = self.expect_punct(Punct::RBracket, "`]`")?;
                let base_span = lhs.span();
                Ok(Expr::Index {
                    base: Box::new(lhs),
                    index: Box::new(index),
                    span: base_span.merge(rbr),
                })
            },
            Punct::PlusPlus | Punct::MinusMinus => {
                let op = if p == Punct::PlusPlus { UnaryOp::PostInc } else { UnaryOp::PostDec };
                let base_span = lhs.span();
                Ok(Expr::Unary {
                    op,
                    expr: Box::new(lhs),
                    span: base_span.merge(op_span),
                })
            },
            _ => unreachable!("postfix_bp returned Some for non-postfix `{p:?}`"),
        }
    }

    fn primary_expr(&mut self) -> Result<Expr, Error> {
        match self.peek_kind() {
            Some(TokenKind::IntLit(_)) => {
                let span = self.peek_span();
                let value = match self.bump().unwrap().kind {
                    TokenKind::IntLit(v) => v,
                    _ => unreachable!(),
                };
                Ok(Expr::IntLit { value, span })
            },
            Some(TokenKind::FloatLit(_)) => {
                let span = self.peek_span();
                let value = match self.bump().unwrap().kind {
                    TokenKind::FloatLit(v) => v,
                    _ => unreachable!(),
                };
                Ok(Expr::FloatLit { value, span })
            },
            Some(TokenKind::Keyword(Keyword::True)) => {
                let span = self.peek_span();
                self.bump();
                Ok(Expr::BoolLit { value: true, span })
            },
            Some(TokenKind::Keyword(Keyword::False)) => {
                let span = self.peek_span();
                self.bump();
                Ok(Expr::BoolLit { value: false, span })
            },
            Some(TokenKind::Punct(Punct::LParen)) => {
                self.bump();
                let inner = self.expr()?;
                self.expect_punct(Punct::RParen, "`)`")?;
                Ok(inner)
            },
            Some(TokenKind::Ident(_)) => {
                let span = self.peek_span();
                let name = match self.bump().unwrap().kind {
                    TokenKind::Ident(s) => s,
                    _ => unreachable!(),
                };
                // Postfix `(` / `.` / `[` / `++` / `--` are handled by
                // [`Self::apply_postfix`] via the Pratt loop.
                Ok(Expr::Ident { name, span })
            },
            // Constructor: `<type-keyword>(args)`. Type keywords cannot
            // stand alone as expressions, so primary eats the whole call
            // shape directly.
            Some(TokenKind::Keyword(k)) if TypeKind::from_keyword(*k).is_some() => {
                let callee_span = self.peek_span();
                let kw = *k;
                self.bump();
                self.expect_punct(Punct::LParen, "`(` (constructor)")?;
                let args = self.call_args()?;
                let rparen = self.expect_punct(Punct::RParen, "`)`")?;
                Ok(Expr::Call {
                    callee: type_keyword_spelling(kw).to_string(),
                    callee_span,
                    args,
                    span: callee_span.merge(rparen),
                })
            },
            Some(k) => Err(Error::new(
                ErrorKind::Expected { wanted: "expression", got: k.label() },
                self.peek_span(),
            )),
            None => Err(Error::new(
                ErrorKind::UnexpectedEof { wanted: "expression" },
                self.peek_span(),
            )),
        }
    }

    fn call_args(&mut self) -> Result<Vec<Expr>, Error> {
        let mut args = Vec::new();
        if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::RParen))) {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            match self.peek_kind() {
                Some(TokenKind::Punct(Punct::Comma)) => {
                    self.bump();
                    continue;
                },
                Some(TokenKind::Punct(Punct::RParen)) => break,
                Some(k) => {
                    return Err(Error::new(
                        ErrorKind::Expected { wanted: "`,` or `)`", got: k.label() },
                        self.peek_span(),
                    ));
                },
                None => {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof { wanted: "`,` or `)`" },
                        self.peek_span(),
                    ));
                },
            }
        }
        Ok(args)
    }
}

// ---------- binding-power tables --------------------------------------
//
// Numbers gapped by 2 so future operators (shift / bitwise / ternary)
// slot in without renumbering.

fn infix_bp(p: Punct) -> Option<(u8, u8)> {
    Some(match p {
        Punct::Assign
        | Punct::PlusEq
        | Punct::MinusEq
        | Punct::StarEq
        | Punct::SlashEq => (2, 1),
        Punct::OrOr => (3, 4),
        Punct::AndAnd => (5, 6),
        Punct::Eq | Punct::Ne => (7, 8),
        Punct::Lt | Punct::Le | Punct::Gt | Punct::Ge => (9, 10),
        Punct::Plus | Punct::Minus => (11, 12),
        Punct::Star | Punct::Slash | Punct::Percent => (13, 14),
        _ => return None,
    })
}

fn prefix_bp(p: Punct) -> Option<u8> {
    Some(match p {
        Punct::Plus | Punct::Minus | Punct::Bang | Punct::PlusPlus | Punct::MinusMinus => 17,
        _ => return None,
    })
}

fn postfix_bp(p: Punct) -> Option<u8> {
    Some(match p {
        Punct::LParen | Punct::Dot | Punct::LBracket | Punct::PlusPlus | Punct::MinusMinus => 19,
        _ => return None,
    })
}

fn prefix_op_from_punct(p: Punct) -> UnaryOp {
    match p {
        Punct::Plus => UnaryOp::Pos,
        Punct::Minus => UnaryOp::Neg,
        Punct::Bang => UnaryOp::Not,
        Punct::PlusPlus => UnaryOp::PreInc,
        Punct::MinusMinus => UnaryOp::PreDec,
        _ => unreachable!("prefix_bp returned Some for non-prefix `{p:?}`"),
    }
}

fn make_infix(p: Punct, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span().merge(rhs.span());
    if let Some(op) = assign_op_from_punct(p) {
        return Expr::Assign { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
    }
    let op = match p {
        Punct::OrOr => BinOp::LogOr,
        Punct::AndAnd => BinOp::LogAnd,
        Punct::Eq => BinOp::Eq,
        Punct::Ne => BinOp::Ne,
        Punct::Lt => BinOp::Lt,
        Punct::Le => BinOp::Le,
        Punct::Gt => BinOp::Gt,
        Punct::Ge => BinOp::Ge,
        Punct::Plus => BinOp::Add,
        Punct::Minus => BinOp::Sub,
        Punct::Star => BinOp::Mul,
        Punct::Slash => BinOp::Div,
        Punct::Percent => BinOp::Rem,
        _ => unreachable!("infix_bp returned Some for non-binary `{p:?}`"),
    };
    Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span }
}

fn assign_op_from_punct(p: Punct) -> Option<AssignOp> {
    Some(match p {
        Punct::Assign => AssignOp::Assign,
        Punct::PlusEq => AssignOp::AddAssign,
        Punct::MinusEq => AssignOp::SubAssign,
        Punct::StarEq => AssignOp::MulAssign,
        Punct::SlashEq => AssignOp::DivAssign,
        _ => return None,
    })
}

fn type_keyword_spelling(k: Keyword) -> &'static str {
    match k {
        Keyword::Void => "void",
        Keyword::Bool => "bool",
        Keyword::Int => "int",
        Keyword::Float => "float",
        Keyword::Vec2 => "vec2",
        Keyword::Vec3 => "vec3",
        Keyword::Vec4 => "vec4",
        Keyword::Mat2 => "mat2",
        Keyword::Mat3 => "mat3",
        Keyword::Mat4 => "mat4",
        Keyword::Sampler2D => "sampler2D",
        Keyword::SamplerCube => "samplerCube",
        _ => "<non-type-keyword>",
    }
}
