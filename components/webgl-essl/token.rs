/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! ESSL token kinds. Spike coverage: the keyword set the canonical triangle
//! shaders plus their immediate corpus (uniform / varying / binary `*`)
//! exercise. Adding a keyword is one row in [`Keyword`] + one arm in the
//! lexer's keyword recognizer.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    Keyword(Keyword),
    Punct(Punct),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Keyword {
    // Storage qualifiers
    Attribute,
    Uniform,
    Varying,
    Const,
    // ESSL 3.00 storage / interpolation qualifiers
    In,
    Out,
    Centroid,
    Flat,
    Smooth,
    // Precision qualifiers
    Precision,
    Lowp,
    Mediump,
    Highp,
    // Type specifiers
    Void,
    Bool,
    Int,
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Sampler2D,
    SamplerCube,
    // Booleans
    True,
    False,
    // Control flow (parsed even when unused by the canonical shaders, so the
    // grammar surface stays consistent across the WebGL 1 corpus)
    If,
    Else,
    For,
    While,
    Do,
    Return,
    Break,
    Continue,
    Discard,
    Struct,
}

impl Keyword {
    pub fn from_word(s: &str) -> Option<Self> {
        use Keyword::*;
        Some(match s {
            "attribute" => Attribute,
            "uniform" => Uniform,
            "varying" => Varying,
            "const" => Const,
            "in" => In,
            "out" => Out,
            "centroid" => Centroid,
            "flat" => Flat,
            "smooth" => Smooth,
            "precision" => Precision,
            "lowp" => Lowp,
            "mediump" => Mediump,
            "highp" => Highp,
            "void" => Void,
            "bool" => Bool,
            "int" => Int,
            "float" => Float,
            "vec2" => Vec2,
            "vec3" => Vec3,
            "vec4" => Vec4,
            "mat2" => Mat2,
            "mat3" => Mat3,
            "mat4" => Mat4,
            "sampler2D" => Sampler2D,
            "samplerCube" => SamplerCube,
            "true" => True,
            "false" => False,
            "if" => If,
            "else" => Else,
            "for" => For,
            "while" => While,
            "do" => Do,
            "return" => Return,
            "break" => Break,
            "continue" => Continue,
            "discard" => Discard,
            "struct" => Struct,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Dot,
    // Assignment
    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    // Logical
    AndAnd,
    OrOr,
    Bang,
    // Bitwise / shift (ESSL 3.00; tokenized but the spike parser rejects
    // them as Unsupported until the corpus needs them)
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    // Ternary
    Question,
    Colon,
    // Increment/decrement
    PlusPlus,
    MinusMinus,
}

impl TokenKind {
    /// One-line label for diagnostics ("identifier `foo`", "`;`", "int literal").
    pub fn label(&self) -> String {
        match self {
            TokenKind::Ident(s) => format!("identifier `{s}`"),
            TokenKind::IntLit(_) => "int literal".into(),
            TokenKind::FloatLit(_) => "float literal".into(),
            TokenKind::Keyword(k) => format!("keyword `{k:?}`"),
            TokenKind::Punct(p) => format!("`{p:?}`"),
        }
    }
}
