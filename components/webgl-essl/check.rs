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

mod registry;
mod typing;

pub use registry::Signature;
use registry::{LookupOutcome, Registry};
use typing::{binary_result, constructor_result, swizzle_result, unary_result};

/// Public entry: run the first typecheck pass over a parsed
/// translation unit. The result holds resolved types keyed by node
/// span plus any diagnostics produced along the way.
pub fn check(tu: &TranslationUnit) -> CheckResult {
    let mut tc = TypeChecker::new();
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
    /// Binary operator does not accept the operand pair under ESSL
    /// rules (e.g., `bool + bool` or `mat3 * mat4`).
    BinaryOpMismatch {
        op: BinOp,
        lhs: TypeKind,
        rhs: TypeKind,
    },
    /// Unary operator does not accept this operand type (e.g., `!float`).
    UnaryOpMismatch { op: UnaryOp, operand: TypeKind },
    /// Assignment LHS / RHS type mismatch.
    AssignTypeMismatch { lhs: TypeKind, rhs: TypeKind },
    /// Ternary condition is not `bool`.
    TernaryCondNotBool { cond: TypeKind },
    /// Ternary then / else_ branches resolve to different types.
    TernaryBranchMismatch { then: TypeKind, else_: TypeKind },
    /// `.field` on a base that is not a vec, or a field that is not a
    /// valid swizzle for the base's component count.
    InvalidSwizzle { base: TypeKind, field: String },
    /// `name(args)` where `name` is not a constructor, not a built-in,
    /// and not in scope as a user-defined function.
    UnknownFunction { name: String },
    /// `name(args)` where overloads of `name` exist but none match the
    /// actual argument types.
    CallSignatureMismatch {
        name: String,
        candidates: Vec<Signature>,
        actual: Vec<TypeKind>,
    },
    /// `.field` access on a struct base where the struct has no
    /// member named `field`.
    UnknownStructField {
        struct_name: Option<String>,
        field: String,
    },
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
            TypeDiagnosticKind::BinaryOpMismatch { op, lhs, rhs } => {
                write!(
                    f,
                    "{line}:{col}: binary `{op:?}` does not accept {lhs:?} and {rhs:?}"
                )
            },
            TypeDiagnosticKind::UnaryOpMismatch { op, operand } => {
                write!(
                    f,
                    "{line}:{col}: unary `{op:?}` does not accept {operand:?}"
                )
            },
            TypeDiagnosticKind::AssignTypeMismatch { lhs, rhs } => {
                write!(f, "{line}:{col}: cannot assign {rhs:?} to {lhs:?}")
            },
            TypeDiagnosticKind::TernaryCondNotBool { cond } => {
                write!(
                    f,
                    "{line}:{col}: ternary condition must be bool, got {cond:?}"
                )
            },
            TypeDiagnosticKind::TernaryBranchMismatch { then, else_ } => {
                write!(
                    f,
                    "{line}:{col}: ternary branches differ: then is {then:?}, else is {else_:?}"
                )
            },
            TypeDiagnosticKind::InvalidSwizzle { base, field } => {
                write!(f, "{line}:{col}: invalid swizzle `.{field}` on {base:?}")
            },
            TypeDiagnosticKind::UnknownFunction { name } => {
                write!(f, "{line}:{col}: unknown function `{name}`")
            },
            TypeDiagnosticKind::CallSignatureMismatch {
                name,
                candidates,
                actual,
            } => {
                let actual_str = actual
                    .iter()
                    .map(|t| format!("{t:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "{line}:{col}: no overload of `{name}` accepts ({actual_str}); {n} candidate{s} known",
                    n = candidates.len(),
                    s = if candidates.len() == 1 { "" } else { "s" },
                )
            },
            TypeDiagnosticKind::UnknownStructField { struct_name, field } => {
                let tag = struct_name.as_deref().unwrap_or("<anonymous>");
                write!(f, "{line}:{col}: struct `{tag}` has no field `{field}`")
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

#[derive(Debug, Clone)]
pub struct ScopeEntry {
    pub ty: TypeKind,
    pub decl_span: Span,
    pub kind: SymbolKind,
    /// Function signature when `kind` is `Function` or `Builtin`; None
    /// for vars / params / globals / scalar built-ins.
    pub signature: Option<Signature>,
}

impl ScopeEntry {
    fn var(ty: TypeKind, decl_span: Span, kind: SymbolKind) -> Self {
        Self {
            ty,
            decl_span,
            kind,
            signature: None,
        }
    }
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

    fn lookup(&self, name: &str) -> Option<&ScopeEntry> {
        self.entries.get(name)
    }
}

// ---------- the typechecker visitor -----------------------------------

struct TypeChecker {
    scopes: Vec<Scope>,
    types: HashMap<Span, TypeKind>,
    diagnostics: Vec<TypeDiagnostic>,
    registry: Registry,
    /// User-defined function signatures, accumulated by the
    /// forward-reference pre-pass. Keyed by name with a `Vec` per
    /// entry so two `float helper(...)` declarations with
    /// distinct parameter types overload rather than overwriting
    /// each other.
    user_functions: HashMap<String, Vec<crate::check::registry::Signature>>,
    /// User-defined struct registry. Index matches
    /// [`TypeKind::Struct`]'s index; each entry stores the field
    /// list in source order. Built at translation-unit Pre time
    /// from `ExternalDecl::Struct` entries.
    structs: Vec<StructEntry>,
    /// Struct tag name → its registry index. Populated alongside
    /// `structs`. Anonymous structs are not registered here.
    struct_name_to_idx: HashMap<String, u32>,
}

#[derive(Clone)]
struct StructEntry {
    /// Optional tag name; matches `StructDecl::name`.
    name: Option<String>,
    /// (field name, field type) pairs in source order.
    fields: Vec<(String, TypeKind)>,
}

impl TypeChecker {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            types: HashMap::new(),
            diagnostics: Vec::new(),
            registry: Registry::with_builtins(),
            user_functions: HashMap::new(),
            structs: Vec::new(),
            struct_name_to_idx: HashMap::new(),
        }
    }

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

    fn lookup(&self, name: &str) -> Option<&ScopeEntry> {
        self.scopes.iter().rev().find_map(|s| s.lookup(name))
    }

    /// Seed the global scope with the special variables every WebGL 1
    /// shader sees. Spec-faithful staging is a Step 4b refinement;
    /// today this just prevents spurious `UnknownIdentifier` noise.
    fn seed_global_builtins(&mut self) {
        let zero = Span::new(0, 0);
        let seed = |this: &mut Self, name: &str, ty: TypeKind| {
            this.define_in_current(name, ScopeEntry::var(ty, zero, SymbolKind::Builtin));
        };
        // Vertex-only outputs (real spec gates these by stage; we don't yet).
        seed(self, "gl_Position", TypeKind::Vec4);
        seed(self, "gl_PointSize", TypeKind::Float);
        // Fragment-only outputs / inputs.
        seed(self, "gl_FragColor", TypeKind::Vec4);
        seed(self, "gl_FragCoord", TypeKind::Vec4);
        seed(self, "gl_PointCoord", TypeKind::Vec2);
        seed(self, "gl_FrontFacing", TypeKind::Bool);
    }

    /// Pre-pass: build the typechecker's struct registry from
    /// `ExternalDecl::Struct` entries. The index assigned here
    /// matches the parser's `TypeKind::Struct(i)`.
    fn collect_structs(&mut self, tu: &TranslationUnit) {
        for decl in &tu.decls {
            if let ExternalDecl::Struct(s) = decl {
                let fields = s
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.kind))
                    .collect();
                let idx = self.structs.len() as u32;
                if let Some(n) = &s.name {
                    self.struct_name_to_idx.insert(n.clone(), idx);
                }
                self.structs.push(StructEntry {
                    name: s.name.clone(),
                    fields,
                });
            }
        }
    }

    /// Pre-pass run at translation-unit Pre to register every user
    /// function in the global scope, so forward references resolve.
    fn register_user_function_signatures(&mut self, tu: &TranslationUnit) {
        for decl in &tu.decls {
            if let ExternalDecl::Function(f) = decl {
                let signature = Signature {
                    params: f.params.iter().map(|p| p.ty.kind).collect(),
                    result: f.return_ty.kind,
                };
                // Track every user function overload by name so
                // Call resolution can pick the matching one. The
                // scope-define is kept too so an Ident referring
                // to a function name still resolves (the scope
                // form holds only one signature; the
                // `user_functions` map holds all overloads).
                self.user_functions
                    .entry(f.name.clone())
                    .or_default()
                    .push(signature.clone());
                self.define_in_current(
                    &f.name,
                    ScopeEntry {
                        ty: f.return_ty.kind,
                        decl_span: f.name_span,
                        kind: SymbolKind::Function,
                        signature: Some(signature),
                    },
                );
            }
        }
    }
}

impl<'tree> Visitor<'tree> for TypeChecker {
    fn visit_translation_unit(&mut self, tu: &'tree TranslationUnit, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                self.push_scope();
                self.seed_global_builtins();
                // Pre-pass: build the struct registry by walking
                // `ExternalDecl::Struct` in source order — the
                // index matches the parser's `TypeKind::Struct(i)`
                // assignment.
                self.collect_structs(tu);
                // Forward-reference pre-pass: register every user
                // function in the global scope so a Call to a function
                // declared later in source order still resolves.
                self.register_user_function_signatures(tu);
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
                ScopeEntry::var(g.ty.kind, g.name_span, SymbolKind::Global),
            );
        }
        Walk::Continue
    }

    fn visit_function_def(&mut self, fd: &'tree FunctionDef, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                // Function name is already in the global scope from the
                // pre-pass in visit_translation_unit; just push the body
                // scope and seed params.
                self.push_scope();
                for p in &fd.params {
                    self.define_in_current(
                        &p.name,
                        ScopeEntry::var(p.ty.kind, p.span, SymbolKind::Param),
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
                ScopeEntry::var(d.ty.kind, d.name_span, SymbolKind::Var),
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
                Expr::Ident { name, span } => {
                    let ty = self.lookup(name).map(|e| e.ty);
                    match ty {
                        Some(ty) => {
                            self.types.insert(*span, ty);
                        },
                        None => {
                            self.diagnostics.push(TypeDiagnostic {
                                kind: TypeDiagnosticKind::UnknownIdentifier { name: name.clone() },
                                span: *span,
                            });
                        },
                    }
                },
                Expr::Binary { op, lhs, rhs, span } => {
                    let lt = self.types.get(&lhs.span()).copied();
                    let rt = self.types.get(&rhs.span()).copied();
                    if let (Some(lt), Some(rt)) = (lt, rt) {
                        match binary_result(*op, lt, rt) {
                            Some(result) => {
                                self.types.insert(*span, result);
                            },
                            None => {
                                self.diagnostics.push(TypeDiagnostic {
                                    kind: TypeDiagnosticKind::BinaryOpMismatch {
                                        op: *op,
                                        lhs: lt,
                                        rhs: rt,
                                    },
                                    span: *span,
                                });
                            },
                        }
                    }
                },
                Expr::Unary { op, expr, span } => {
                    if let Some(t) = self.types.get(&expr.span()).copied() {
                        match unary_result(*op, t) {
                            Some(result) => {
                                self.types.insert(*span, result);
                            },
                            None => {
                                self.diagnostics.push(TypeDiagnostic {
                                    kind: TypeDiagnosticKind::UnaryOpMismatch {
                                        op: *op,
                                        operand: t,
                                    },
                                    span: *span,
                                });
                            },
                        }
                    }
                },
                Expr::Assign { op, lhs, rhs, span } => {
                    // Result of an assignment is the LHS's type.
                    // For compound assigns, the effective rhs is
                    // `binary_result(lhs, <op>, rhs)` — so
                    // `vec3 *= float` is legal because
                    // `binary_result(Vec3, Mul, Float) == Vec3`.
                    let lt = self.types.get(&lhs.span()).copied();
                    let rt = self.types.get(&rhs.span()).copied();
                    if let Some(lt) = lt {
                        self.types.insert(*span, lt);
                        if let Some(rt) = rt {
                            let effective_rhs = match op {
                                AssignOp::Assign => Some(rt),
                                AssignOp::AddAssign => binary_result(BinOp::Add, lt, rt),
                                AssignOp::SubAssign => binary_result(BinOp::Sub, lt, rt),
                                AssignOp::MulAssign => binary_result(BinOp::Mul, lt, rt),
                                AssignOp::DivAssign => binary_result(BinOp::Div, lt, rt),
                            };
                            match effective_rhs {
                                Some(eff) if eff == lt => {},
                                _ => {
                                    self.diagnostics.push(TypeDiagnostic {
                                        kind: TypeDiagnosticKind::AssignTypeMismatch {
                                            lhs: lt,
                                            rhs: rt,
                                        },
                                        span: *span,
                                    });
                                },
                            }
                        }
                    }
                },
                Expr::Ternary {
                    cond,
                    then,
                    else_,
                    span,
                } => {
                    let ct = self.types.get(&cond.span()).copied();
                    let tt = self.types.get(&then.span()).copied();
                    let et = self.types.get(&else_.span()).copied();
                    if let Some(ct) = ct {
                        if ct != TypeKind::Bool {
                            self.diagnostics.push(TypeDiagnostic {
                                kind: TypeDiagnosticKind::TernaryCondNotBool { cond: ct },
                                span: *span,
                            });
                        }
                    }
                    if let (Some(tt), Some(et)) = (tt, et) {
                        if tt == et {
                            self.types.insert(*span, tt);
                        } else {
                            self.diagnostics.push(TypeDiagnostic {
                                kind: TypeDiagnosticKind::TernaryBranchMismatch {
                                    then: tt,
                                    else_: et,
                                },
                                span: *span,
                            });
                        }
                    }
                },
                Expr::Member {
                    base, field, span, ..
                } => {
                    if let Some(bt) = self.types.get(&base.span()).copied() {
                        // Dispatch on base kind: struct → field
                        // lookup; vector → swizzle. Anything else
                        // becomes InvalidSwizzle.
                        if let TypeKind::Struct(idx) = bt {
                            let entry = &self.structs[idx as usize];
                            let resolved = entry
                                .fields
                                .iter()
                                .find(|(n, _)| n == field)
                                .map(|(_, ty)| *ty);
                            match resolved {
                                Some(field_ty) => {
                                    self.types.insert(*span, field_ty);
                                },
                                None => {
                                    self.diagnostics.push(TypeDiagnostic {
                                        kind: TypeDiagnosticKind::UnknownStructField {
                                            struct_name: entry.name.clone(),
                                            field: field.clone(),
                                        },
                                        span: *span,
                                    });
                                },
                            }
                        } else {
                            match swizzle_result(bt, field) {
                                Some(result) => {
                                    self.types.insert(*span, result);
                                },
                                None => {
                                    self.diagnostics.push(TypeDiagnostic {
                                        kind: TypeDiagnosticKind::InvalidSwizzle {
                                            base: bt,
                                            field: field.clone(),
                                        },
                                        span: *span,
                                    });
                                },
                            }
                        }
                    }
                },
                Expr::Call {
                    callee, args, span, ..
                } => {
                    // Three-stage Call resolution:
                    //   1. Constructor (vec_n / mat_n / scalar) by
                    //      structural rule in `typing::constructor_result`.
                    //   2. Built-in registry lookup against the §8 table.
                    //   3. User-defined function in the scope stack
                    //      (registered up-front by the forward-ref pre-pass).
                    // If all three miss the diagnostic differentiates between
                    // "name unknown" and "name known but no overload accepts
                    // these args".
                    let arg_types: Option<Vec<TypeKind>> = args
                        .iter()
                        .map(|a| self.types.get(&a.span()).copied())
                        .collect();
                    let arg_types = match arg_types {
                        Some(v) => v,
                        None => return Walk::Continue,
                    };

                    // 1. Constructor.
                    if let Some(result) = constructor_result(callee, &arg_types) {
                        self.types.insert(*span, result);
                        return Walk::Continue;
                    }

                    // 1b. Struct constructor. `Foo(args)` builds
                    // a struct of type `Foo` when the arg types
                    // match the declared field types in order.
                    if let Some(&struct_idx) = self.struct_name_to_idx.get(callee) {
                        let entry = &self.structs[struct_idx as usize];
                        let field_kinds: Vec<TypeKind> =
                            entry.fields.iter().map(|(_, t)| *t).collect();
                        if arg_types == field_kinds {
                            self.types.insert(*span, TypeKind::Struct(struct_idx));
                            return Walk::Continue;
                        }
                        // Argument-list mismatch — reuse the
                        // CallSignatureMismatch diagnostic with a
                        // synthetic single candidate matching the
                        // struct's field types.
                        let synthetic = Signature {
                            params: field_kinds,
                            result: TypeKind::Struct(struct_idx),
                        };
                        self.diagnostics.push(TypeDiagnostic {
                            kind: TypeDiagnosticKind::CallSignatureMismatch {
                                name: callee.clone(),
                                candidates: vec![synthetic],
                                actual: arg_types,
                            },
                            span: *span,
                        });
                        return Walk::Continue;
                    }

                    // 2. Built-in registry.
                    match self.registry.lookup(callee, &arg_types) {
                        LookupOutcome::Match(sig) => {
                            self.types.insert(*span, sig.result);
                            return Walk::Continue;
                        },
                        LookupOutcome::Mismatch(candidates) => {
                            self.diagnostics.push(TypeDiagnostic {
                                kind: TypeDiagnosticKind::CallSignatureMismatch {
                                    name: callee.clone(),
                                    candidates: candidates.to_vec(),
                                    actual: arg_types,
                                },
                                span: *span,
                            });
                            return Walk::Continue;
                        },
                        LookupOutcome::Unknown => {},
                    }

                    // 3. User-defined function via the overload set.
                    let overloads = self.user_functions.get(callee).cloned();
                    match overloads {
                        Some(sigs) => match sigs.iter().find(|s| s.matches(&arg_types)) {
                            Some(sig) => {
                                self.types.insert(*span, sig.result);
                            },
                            None => {
                                self.diagnostics.push(TypeDiagnostic {
                                    kind: TypeDiagnosticKind::CallSignatureMismatch {
                                        name: callee.clone(),
                                        candidates: sigs,
                                        actual: arg_types,
                                    },
                                    span: *span,
                                });
                            },
                        },
                        None => {
                            // Synthetic computed callee from a postfix
                            // `(` on a non-ident base passes through
                            // here too; don't emit a noisy diagnostic
                            // for that case.
                            if callee != "<computed>" {
                                self.diagnostics.push(TypeDiagnostic {
                                    kind: TypeDiagnosticKind::UnknownFunction {
                                        name: callee.clone(),
                                    },
                                    span: *span,
                                });
                            }
                        },
                    }
                },
                // Index expressions are deferred (Step 4b second chunk):
                // need component-type rules for vec / array / matrix.
                Expr::Index { .. } => {},
            }
        }
        Walk::Continue
    }
}
