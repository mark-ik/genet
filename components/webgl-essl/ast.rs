/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! ESSL AST. Spike coverage: enough surface to round-trip the canonical
//! triangle shaders plus uniform / varying / binary `*` expressions.
//! Type-check and validate live above this; the AST itself stays a faithful
//! parse tree, not a desugared IR.

use crate::span::Span;
use crate::token::Keyword;

#[derive(Debug, Clone, PartialEq)]
pub struct TranslationUnit {
    pub decls: Vec<ExternalDecl>,
    pub span: Span,
    /// Numeric version from a `#version <N> <profile>` directive, if
    /// any. `Some(300)` for `#version 300 es` (ESSL 3.00), `Some(100)`
    /// for `#version 100` (ESSL 1.00, the WebGL 1 default), `None`
    /// when no directive is present (caller's stage / spec default
    /// applies).
    pub version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExternalDecl {
    /// `precision <q> <type>;`
    Precision(PrecisionDecl),
    /// `<storage> <type> <name>;` at file scope.
    Global(GlobalDecl),
    /// `<return-type> <name>(<params>) { <body> }`
    Function(FunctionDef),
    /// `struct <name>? { <fields> };` at file scope.
    Struct(StructDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    /// Tag name. `None` is allowed by ESSL when the struct is used
    /// inline as a type specifier, though the spike's parser always
    /// expects one here.
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub fields: Vec<StructField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub ty: TypeSpec,
    pub name: String,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrecisionDecl {
    pub qualifier: PrecisionQualifier,
    pub ty: TypeSpec,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalDecl {
    pub storage: StorageQualifier,
    pub precision: Option<PrecisionQualifier>,
    pub ty: TypeSpec,
    pub name: String,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub return_ty: TypeSpec,
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub ty: TypeSpec,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StorageQualifier {
    /// No qualifier in source — local variable, or a function-scope decl.
    None,
    /// `attribute` — vertex-shader input. ESSL 1.00.
    Attribute,
    /// `uniform` — pipeline-constant input.
    Uniform,
    /// `varying` — vertex-to-fragment interpolated value. ESSL 1.00.
    Varying,
    /// `const` — compile-time constant.
    Const,
    /// `in` — stage input. ESSL 3.00. Replaces `attribute` and the
    /// in-direction of `varying`.
    In,
    /// `out` — stage output. ESSL 3.00. Replaces the out-direction of
    /// `varying`.
    Out,
    /// `centroid` — interpolation modifier (centroid sampling).
    /// ESSL 3.00.
    Centroid,
    /// `flat` — interpolation modifier (no interpolation). ESSL 3.00.
    Flat,
    /// `smooth` — interpolation modifier (default behavior, named for
    /// clarity). ESSL 3.00.
    Smooth,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PrecisionQualifier {
    Low,
    Medium,
    High,
}

impl PrecisionQualifier {
    pub fn from_keyword(k: Keyword) -> Option<Self> {
        match k {
            Keyword::Lowp => Some(Self::Low),
            Keyword::Mediump => Some(Self::Medium),
            Keyword::Highp => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypeSpec {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TypeKind {
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
}

impl TypeKind {
    pub fn from_keyword(k: Keyword) -> Option<Self> {
        Some(match k {
            Keyword::Void => Self::Void,
            Keyword::Bool => Self::Bool,
            Keyword::Int => Self::Int,
            Keyword::Float => Self::Float,
            Keyword::Vec2 => Self::Vec2,
            Keyword::Vec3 => Self::Vec3,
            Keyword::Vec4 => Self::Vec4,
            Keyword::Mat2 => Self::Mat2,
            Keyword::Mat3 => Self::Mat3,
            Keyword::Mat4 => Self::Mat4,
            Keyword::Sampler2D => Self::Sampler2D,
            Keyword::SamplerCube => Self::SamplerCube,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    /// `return [expr];`
    Return { value: Option<Expr>, span: Span },
    /// `<type> <name>[ = <init>];` at block scope.
    Decl(LocalDecl),
    /// `{ <stmts> }` as a statement (compound).
    Block(Block),
    /// `if (<cond>) <then> [else <else_>]`
    If { cond: Expr, then: Box<Stmt>, else_: Option<Box<Stmt>>, span: Span },
    /// `while (<cond>) <body>`
    While { cond: Expr, body: Box<Stmt>, span: Span },
    /// `for (<init> <cond>; <step>) <body>`. `init` carries its own
    /// trailing `;`.
    For {
        init: ForInit,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
        span: Span,
    },
    /// `do <body> while (<cond>);`
    Do { body: Box<Stmt>, cond: Expr, span: Span },
    /// `break;`
    Break { span: Span },
    /// `continue;`
    Continue { span: Span },
    /// `discard;` — fragment-shader only; parser accepts everywhere.
    Discard { span: Span },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Expr(e) => e.span(),
            Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Do { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span }
            | Stmt::Discard { span } => *span,
            Stmt::Decl(d) => d.span,
            Stmt::Block(b) => b.span,
        }
    }
}

/// Initializer slot of a `for` loop. The semicolon between `init` and
/// `cond` is consumed by whichever variant carries it (the [`LocalDecl`]
/// and the expression form both eat their own trailing `;`).
#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    Empty,
    Decl(LocalDecl),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalDecl {
    pub is_const: bool,
    pub precision: Option<PrecisionQualifier>,
    pub ty: TypeSpec,
    pub name: String,
    pub name_span: Span,
    pub init: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    IntLit { value: i64, span: Span },
    FloatLit { value: f64, span: Span },
    BoolLit { value: bool, span: Span },
    Ident { name: String, span: Span },
    /// `callee(arg, arg, ...)`. ESSL constructors (`vec4(...)`) and function
    /// calls share this shape; the validator distinguishes them later via
    /// the type/symbol table.
    Call { callee: String, callee_span: Span, args: Vec<Expr>, span: Span },
    /// `<lhs> = <rhs>`, `<lhs> += <rhs>`, etc.
    Assign { op: AssignOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `<lhs> <op> <rhs>`. Arithmetic, comparison, logical.
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// `<op> <expr>` for prefix ops, or `<expr> <op>` for postfix
    /// `++` / `--`. The validator enforces the prefix-vs-postfix
    /// difference on the same `UnaryOp` enum.
    Unary { op: UnaryOp, expr: Box<Expr>, span: Span },
    /// `<base>.<field>`. ESSL swizzles (`.xyz`) and struct field access
    /// both lower to this; the validator picks an interpretation by
    /// inspecting `base`'s type.
    Member { base: Box<Expr>, field: String, field_span: Span, span: Span },
    /// `<base>[<index>]` — array or vector subscript.
    Index { base: Box<Expr>, index: Box<Expr>, span: Span },
    /// `<cond> ? <then> : <else_>`. Right-associative; precedence sits
    /// between assignment (looser) and logical-or (tighter).
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `+x` (rare but ESSL allows it)
    Pos,
    /// `!x`
    Not,
    /// `~x` — bitwise complement. ESSL 3.00 only on integer operands.
    BitNot,
    /// `++x`
    PreInc,
    /// `--x`
    PreDec,
    /// `x++`
    PostInc,
    /// `x--`
    PostDec,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LogAnd,
    LogOr,
    /// `<<` — left shift. ESSL 3.00 only.
    Shl,
    /// `>>` — right shift. ESSL 3.00 only.
    Shr,
    /// `&` — bitwise AND. ESSL 3.00 only.
    BitAnd,
    /// `|` — bitwise OR. ESSL 3.00 only.
    BitOr,
    /// `^` — bitwise XOR. ESSL 3.00 only.
    BitXor,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit { span, .. }
            | Expr::FloatLit { span, .. }
            | Expr::BoolLit { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Call { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Member { span, .. }
            | Expr::Index { span, .. }
            | Expr::Ternary { span, .. } => *span,
        }
    }
}
