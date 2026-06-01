/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Diagnostic types. Spike-grade: one error kind per failure shape, carrying
//! a span. ANGLE-shaped error codes (`getError()` discipline) are a later
//! layer above this.

use std::fmt;

use crate::span::{Span, line_column};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ErrorKind {
    /// Lexer hit a byte it has no token for.
    UnexpectedByte(u8),
    /// Lexer found a number that doesn't fit ESSL int / float shape.
    MalformedNumber,
    /// Parser wanted a specific token and got something else.
    Expected { wanted: &'static str, got: String },
    /// Parser reached end of input where more was required.
    UnexpectedEof { wanted: &'static str },
    /// Parser saw a syntactically-valid construct that isn't yet
    /// recognized by this spike's grammar coverage.
    Unsupported { what: &'static str },
}

impl Error {
    pub fn new(kind: ErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn display<'a>(&'a self, src: &'a str) -> ErrorDisplay<'a> {
        ErrorDisplay { err: self, src }
    }
}

pub struct ErrorDisplay<'a> {
    err: &'a Error,
    src: &'a str,
}

impl fmt::Display for ErrorDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (line, col) = line_column(self.src, self.err.span.start);
        match &self.err.kind {
            ErrorKind::UnexpectedByte(b) => {
                write!(f, "{line}:{col}: unexpected byte 0x{b:02x}")
            },
            ErrorKind::MalformedNumber => write!(f, "{line}:{col}: malformed number literal"),
            ErrorKind::Expected { wanted, got } => {
                write!(f, "{line}:{col}: expected {wanted}, got {got}")
            },
            ErrorKind::UnexpectedEof { wanted } => {
                write!(f, "{line}:{col}: unexpected end of input, expected {wanted}")
            },
            ErrorKind::Unsupported { what } => {
                write!(f, "{line}:{col}: spike does not yet support {what}")
            },
        }
    }
}
