/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! External declarations: precision, globals, functions, and the helpers
//! that recognize types / storage qualifiers / precision qualifiers.

use crate::ast::{
    ExternalDecl, FunctionDef, GlobalDecl, Param, PrecisionDecl, PrecisionQualifier,
    StorageQualifier, StructDecl, StructField, TypeKind, TypeSpec,
};
use crate::error::{Error, ErrorKind};
use crate::span::Span;
use crate::token::{Keyword, Punct, TokenKind};

use super::Parser;

impl Parser {
    pub(super) fn external_decl(&mut self) -> Result<ExternalDecl, Error> {
        if let Some(start) = self.eat_keyword(Keyword::Precision) {
            return self.precision_decl(start);
        }
        if let Some(start) = self.eat_keyword(Keyword::Struct) {
            return self.struct_decl(start);
        }
        if let Some(storage) = self.peek_storage_qualifier() {
            return self.global_decl(storage);
        }
        // A type starts either a function definition or a global decl
        // without a storage qualifier. The next-after-name token picks:
        // `(` => function, `;` => global.
        self.type_starting_decl()
    }

    fn struct_decl(&mut self, start: Span) -> Result<ExternalDecl, Error> {
        let (name, name_span) = match self.peek_kind() {
            Some(TokenKind::Ident(_)) => {
                let (n, s) = self.expect_ident("struct name")?;
                (Some(n), Some(s))
            },
            _ => (None, None),
        };
        self.expect_punct(Punct::LBrace, "`{` (struct body)")?;
        let mut fields = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokenKind::Punct(Punct::RBrace)) | None => break,
                _ => self.struct_field_line(&mut fields)?,
            }
        }
        self.expect_punct(Punct::RBrace, "`}` (struct body)")?;
        let semi = self.expect_punct(Punct::Semi, "`;` after struct")?;
        Ok(ExternalDecl::Struct(StructDecl {
            name,
            name_span,
            fields,
            span: start.merge(semi),
        }))
    }

    fn struct_field_line(&mut self, fields: &mut Vec<StructField>) -> Result<(), Error> {
        // `<type> <name> [, <name>]* ;` — multiple fields can share one type.
        let ty = self.type_spec()?;
        loop {
            let (name, name_span) = self.expect_ident("field name")?;
            let field_span = ty.span.merge(name_span);
            fields.push(StructField { ty: ty.clone(), name, name_span, span: field_span });
            match self.peek_kind() {
                Some(TokenKind::Punct(Punct::Comma)) => {
                    self.bump();
                    continue;
                },
                Some(TokenKind::Punct(Punct::Semi)) => {
                    self.bump();
                    return Ok(());
                },
                Some(k) => {
                    return Err(Error::new(
                        ErrorKind::Expected { wanted: "`,` or `;`", got: k.label() },
                        self.peek_span(),
                    ));
                },
                None => {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof { wanted: "`,` or `;`" },
                        self.peek_span(),
                    ));
                },
            }
        }
    }

    fn precision_decl(&mut self, kw_span: Span) -> Result<ExternalDecl, Error> {
        let q = match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => PrecisionQualifier::from_keyword(*k).ok_or_else(|| {
                Error::new(
                    ErrorKind::Expected {
                        wanted: "precision qualifier",
                        got: self.peek_kind().unwrap().label(),
                    },
                    self.peek_span(),
                )
            })?,
            Some(k) => {
                return Err(Error::new(
                    ErrorKind::Expected { wanted: "precision qualifier", got: k.label() },
                    self.peek_span(),
                ));
            },
            None => {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof { wanted: "precision qualifier" },
                    self.peek_span(),
                ));
            },
        };
        self.bump();
        let ty = self.type_spec()?;
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        Ok(ExternalDecl::Precision(PrecisionDecl {
            qualifier: q,
            ty,
            span: kw_span.merge(semi),
        }))
    }

    pub(super) fn peek_storage_qualifier(&self) -> Option<(StorageQualifier, Span)> {
        match self.peek_kind() {
            Some(TokenKind::Keyword(Keyword::Attribute)) => {
                Some((StorageQualifier::Attribute, self.peek_span()))
            },
            Some(TokenKind::Keyword(Keyword::Uniform)) => {
                Some((StorageQualifier::Uniform, self.peek_span()))
            },
            Some(TokenKind::Keyword(Keyword::Varying)) => {
                Some((StorageQualifier::Varying, self.peek_span()))
            },
            Some(TokenKind::Keyword(Keyword::Const)) => {
                Some((StorageQualifier::Const, self.peek_span()))
            },
            _ => None,
        }
    }

    fn global_decl(
        &mut self,
        storage: (StorageQualifier, Span),
    ) -> Result<ExternalDecl, Error> {
        let (sq, start_span) = storage;
        self.bump();
        let precision = self.maybe_precision_qualifier();
        let ty = self.type_spec()?;
        let (name, name_span) = self.expect_ident("variable name")?;
        let semi = self.expect_punct(Punct::Semi, "`;`")?;
        Ok(ExternalDecl::Global(GlobalDecl {
            storage: sq,
            precision,
            ty,
            name,
            name_span,
            span: start_span.merge(semi),
        }))
    }

    fn type_starting_decl(&mut self) -> Result<ExternalDecl, Error> {
        let ty = self.type_spec()?;
        let (name, name_span) = self.expect_ident("function or variable name")?;
        match self.peek_kind() {
            Some(TokenKind::Punct(Punct::LParen)) => self.function_def_tail(ty, name, name_span),
            Some(TokenKind::Punct(Punct::Semi)) => {
                let semi = self.peek_span();
                self.bump();
                Ok(ExternalDecl::Global(GlobalDecl {
                    storage: StorageQualifier::None,
                    precision: None,
                    ty: ty.clone(),
                    name,
                    name_span,
                    span: ty.span.merge(semi),
                }))
            },
            Some(k) => Err(Error::new(
                ErrorKind::Expected {
                    wanted: "`(` (function) or `;` (variable)",
                    got: k.label(),
                },
                self.peek_span(),
            )),
            None => Err(Error::new(
                ErrorKind::UnexpectedEof { wanted: "`(` or `;`" },
                self.peek_span(),
            )),
        }
    }

    fn function_def_tail(
        &mut self,
        return_ty: TypeSpec,
        name: String,
        name_span: Span,
    ) -> Result<ExternalDecl, Error> {
        let start = return_ty.span;
        self.expect_punct(Punct::LParen, "`(`")?;
        let params = self.parse_params()?;
        self.expect_punct(Punct::RParen, "`)`")?;
        let body = self.block()?;
        let body_span = body.span;
        Ok(ExternalDecl::Function(FunctionDef {
            return_ty,
            name,
            name_span,
            params,
            body,
            span: start.merge(body_span),
        }))
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, Error> {
        if matches!(self.peek_kind(), Some(TokenKind::Punct(Punct::RParen))) {
            return Ok(Vec::new());
        }
        // `(void)` is the explicit-empty form; one-token lookahead picks
        // it without consuming `void` if it's actually a first-param type.
        if matches!(self.peek_kind(), Some(TokenKind::Keyword(Keyword::Void))) {
            if matches!(self.peek_at(1), Some(TokenKind::Punct(Punct::RParen))) {
                self.bump();
                return Ok(Vec::new());
            }
        }
        let mut params = Vec::new();
        loop {
            let ty = self.type_spec()?;
            let (name, name_span) = self.expect_ident("parameter name")?;
            let ty_span = ty.span;
            params.push(Param { ty, name, span: ty_span.merge(name_span) });
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
        Ok(params)
    }

    pub(super) fn maybe_precision_qualifier(&mut self) -> Option<PrecisionQualifier> {
        match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => {
                if let Some(q) = PrecisionQualifier::from_keyword(*k) {
                    self.bump();
                    Some(q)
                } else {
                    None
                }
            },
            _ => None,
        }
    }

    pub(super) fn type_spec(&mut self) -> Result<TypeSpec, Error> {
        match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => {
                if let Some(tk) = TypeKind::from_keyword(*k) {
                    let span = self.peek_span();
                    self.bump();
                    Ok(TypeSpec { kind: tk, span })
                } else {
                    Err(Error::new(
                        ErrorKind::Expected { wanted: "type", got: self.peek_kind().unwrap().label() },
                        self.peek_span(),
                    ))
                }
            },
            Some(k) => Err(Error::new(
                ErrorKind::Expected { wanted: "type", got: k.label() },
                self.peek_span(),
            )),
            None => Err(Error::new(
                ErrorKind::UnexpectedEof { wanted: "type" },
                self.peek_span(),
            )),
        }
    }
}
