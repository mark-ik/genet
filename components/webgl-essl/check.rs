/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! First typecheck pass: symbol resolution, literal types, identifier
//! types.
//!
//! Implements [`Visitor`] over the parse AST, carrying a scope stack
//! and a `HashMap<Span, TypeKind>` of resolved types. Emits
//! [`TypeDiagnostic`]s for unresolved identifiers; binary-op result
//! types, l-value rules, constructor signatures, and the built-in
//! function registry are Step 4b work (per the design sketch
//! `serval/docs/2026-05-28_webgl_essl_typecheck_visitor_design.md`).
//!
//! ANGLE-shaped `getError()` diagnostics arrive at Step 5 (the WebGL
//! validator layer above this pass); the diagnostics this module emits
//! are the parser-side equivalents.

use std::collections::HashMap;
use std::fmt;

use crate::ast::*;
use crate::span::{Span, line_column};
use crate::visit::{Visit, Visitor, Walk, walk_translation_unit};

/// Public entry: run the first typecheck pass over a parsed
/// translation unit. The result holds resolved types keyed by node
/// span plus any diagnostics produced along the way.
pub fn check(tu: &TranslationUnit) -> CheckResult {
    let mut tc = TypeChecker::default();
    walk_translation_unit(&mut tc, tu);
    CheckResult {
        types: tc.types,
        diagnostics: tc.diagnostics,
    }
}

#[derive(Debug, Default)]
pub struct CheckResult {
    /// Resolved type for each annotated node, keyed by the node's
    /// span. Spans are unique enough for the AST shapes this pass
    /// touches; if a collision is ever possible (e.g., the parser
    /// emits a synthetic node sharing a real node's span) the key
    /// will need to grow into a node id.
    pub types: HashMap<Span, TypeKind>,
    pub diagnostics: Vec<TypeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDiagnostic {
    pub kind: TypeDiagnosticKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDiagnosticKind {
    /// Identifier appears in expression position without a matching
    /// declaration in the scope stack at that point.
    UnknownIdentifier { name: String },
}

impl TypeDiagnostic {
    pub fn display<'a>(&'a self, src: &'a str) -> DiagnosticDisplay<'a> {
        DiagnosticDisplay { diag: self, src }
    }
}

pub struct DiagnosticDisplay<'a> {
    diag: &'a TypeDiagnostic,
    src: &'a str,
}

impl fmt::Display for DiagnosticDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (line, col) = line_column(self.src, self.diag.span.start);
        match &self.diag.kind {
            TypeDiagnosticKind::UnknownIdentifier { name } => {
                write!(f, "{line}:{col}: unknown identifier `{name}`")
            },
        }
    }
}

// ---------- symbol table ----------------------------------------------

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SymbolKind {
    Var,
    Param,
    Global,
    Function,
    Builtin,
}

#[derive(Debug, Clone, Copy)]
pub struct ScopeEntry {
    pub ty: TypeKind,
    pub decl_span: Span,
    pub kind: SymbolKind,
}

#[derive(Default)]
struct Scope {
    entries: HashMap<String, ScopeEntry>,
}

impl Scope {
    fn define(&mut self, name: &str, entry: ScopeEntry) {
        // First-pass policy: overwrite on redeclaration. ESSL forbids
        // same-scope redeclaration; that's a Step 4b diagnostic.
        self.entries.insert(name.to_string(), entry);
    }

    fn lookup(&self, name: &str) -> Option<ScopeEntry> {
        self.entries.get(name).copied()
    }
}

// ---------- the typechecker visitor -----------------------------------

#[derive(Default)]
struct TypeChecker {
    scopes: Vec<Scope>,
    types: HashMap<Span, TypeKind>,
    diagnostics: Vec<TypeDiagnostic>,
}

impl TypeChecker {
    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_in_current(&mut self, name: &str, entry: ScopeEntry) {
        if let Some(top) = self.scopes.last_mut() {
            top.define(name, entry);
        }
    }

    fn lookup(&self, name: &str) -> Option<ScopeEntry> {
        self.scopes.iter().rev().find_map(|s| s.lookup(name))
    }

    /// Seed the global scope with the special variables every WebGL 1
    /// shader sees. Spec-faithful staging is a Step 4b refinement;
    /// today this just prevents spurious `UnknownIdentifier` noise.
    fn populate_builtins(&mut self) {
        let zero = Span::new(0, 0);
        let mut seed = |name: &str, ty: TypeKind| {
            self.define_in_current(name, ScopeEntry { ty, decl_span: zero, kind: SymbolKind::Builtin });
        };
        // Vertex-only outputs (real spec gates these by stage; we don't yet).
        seed("gl_Position", TypeKind::Vec4);
        seed("gl_PointSize", TypeKind::Float);
        // Fragment-only outputs / inputs.
        seed("gl_FragColor", TypeKind::Vec4);
        seed("gl_FragCoord", TypeKind::Vec4);
        seed("gl_PointCoord", TypeKind::Vec2);
        seed("gl_FrontFacing", TypeKind::Bool);
    }
}

impl<'tree> Visitor<'tree> for TypeChecker {
    fn visit_translation_unit(&mut self, _: &'tree TranslationUnit, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                self.push_scope();
                self.populate_builtins();
            },
            Visit::Post => self.pop_scope(),
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_global_decl(&mut self, g: &'tree GlobalDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            self.define_in_current(
                &g.name,
                ScopeEntry { ty: g.ty.kind, decl_span: g.name_span, kind: SymbolKind::Global },
            );
        }
        Walk::Continue
    }

    fn visit_function_def(&mut self, fd: &'tree FunctionDef, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                // Define the function name in the enclosing scope so
                // sibling functions can reference it.
                self.define_in_current(
                    &fd.name,
                    ScopeEntry {
                        ty: fd.return_ty.kind,
                        decl_span: fd.name_span,
                        kind: SymbolKind::Function,
                    },
                );
                // Push the function's own scope and seed params.
                self.push_scope();
                for p in &fd.params {
                    self.define_in_current(
                        &p.name,
                        ScopeEntry { ty: p.ty.kind, decl_span: p.span, kind: SymbolKind::Param },
                    );
                }
            },
            Visit::Post => self.pop_scope(),
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_block(&mut self, _: &'tree Block, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => self.push_scope(),
            Visit::Post => self.pop_scope(),
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_local_decl(&mut self, d: &'tree LocalDecl, visit: Visit) -> Walk {
        // Define on Post so the init expression sees the enclosing
        // value of `name`, not the new one (matches C/Java; ESSL spec
        // is on the same page).
        if visit == Visit::Post {
            self.define_in_current(
                &d.name,
                ScopeEntry { ty: d.ty.kind, decl_span: d.name_span, kind: SymbolKind::Var },
            );
        }
        Walk::Continue
    }

    fn visit_expr(&mut self, e: &'tree Expr, visit: Visit) -> Walk {
        if visit == Visit::Post {
            match e {
                Expr::IntLit { span, .. } => {
                    self.types.insert(*span, TypeKind::Int);
                },
                Expr::FloatLit { span, .. } => {
                    self.types.insert(*span, TypeKind::Float);
                },
                Expr::BoolLit { span, .. } => {
                    self.types.insert(*span, TypeKind::Bool);
                },
                Expr::Ident { name, span } => match self.lookup(name) {
                    Some(entry) => {
                        self.types.insert(*span, entry.ty);
                    },
                    None => {
                        self.diagnostics.push(TypeDiagnostic {
                            kind: TypeDiagnosticKind::UnknownIdentifier { name: name.clone() },
                            span: *span,
                        });
                    },
                },
                // Other expression kinds are not typed by the first
                // pass; binary-op result types and constructor /
                // function-call signatures come in Step 4b.
                _ => {},
            }
        }
        Walk::Continue
    }
}
