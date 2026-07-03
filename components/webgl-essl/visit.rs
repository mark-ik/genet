/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Tree walker + visitor trait for the parse AST.
//!
//! Three-phase visit (Pre / In / Post) modeled on mozangle's
//! `TIntermTraverser` (cargo registry path
//! `mozangle-0.5.5/gfx/angle/checkout/src/compiler/translator/tree_util/`).
//! The walker stays stateless; path tracking, scope stacks, diagnostic
//! collection all live on the visitor.
//!
//! Design rationale: [`serval/docs/2026-05-28_webgl_essl_typecheck_visitor_design.md`](../../docs/2026-05-28_webgl_essl_typecheck_visitor_design.md).

use crate::ast::*;

/// Three-phase visit position. `Pre` fires before descending into
/// children; `Post` fires after the subtree finishes; `In` fires
/// between meaningful child boundaries (between then / else of an
/// `if`, between cond / body of `while`, between siblings of a block
/// or call-args list).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Visit {
    Pre,
    In,
    Post,
}

/// Walk control returned from a Pre-phase visit. `Skip` prunes the
/// subtree; from In or Post phases the return is ignored (matches
/// `TIntermTraverser`'s "false from PreVisit skips children" contract
/// with the polarity inverted to read naturally).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Walk {
    Continue,
    Skip,
}

/// Override the methods for the node kinds you care about; defaults
/// return [`Walk::Continue`]. The `'tree` lifetime threads the parse
/// tree's borrow through every method, so visitors can hold
/// references to nodes in their own state without cloning.
pub trait Visitor<'tree> {
    fn visit_translation_unit(&mut self, _node: &'tree TranslationUnit, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_precision_decl(&mut self, _node: &'tree PrecisionDecl, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_global_decl(&mut self, _node: &'tree GlobalDecl, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_function_def(&mut self, _node: &'tree FunctionDef, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_struct_decl(&mut self, _node: &'tree StructDecl, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_block(&mut self, _node: &'tree Block, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_stmt(&mut self, _node: &'tree Stmt, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_local_decl(&mut self, _node: &'tree LocalDecl, _visit: Visit) -> Walk {
        Walk::Continue
    }
    fn visit_expr(&mut self, _node: &'tree Expr, _visit: Visit) -> Walk {
        Walk::Continue
    }
}

// ---------- walk functions ---------------------------------------------

pub fn walk_translation_unit<'tree, V: Visitor<'tree>>(v: &mut V, tu: &'tree TranslationUnit) {
    if matches!(v.visit_translation_unit(tu, Visit::Pre), Walk::Skip) {
        return;
    }
    let n = tu.decls.len();
    for (i, decl) in tu.decls.iter().enumerate() {
        walk_external_decl(v, decl);
        if i + 1 < n {
            v.visit_translation_unit(tu, Visit::In);
        }
    }
    v.visit_translation_unit(tu, Visit::Post);
}

pub fn walk_external_decl<'tree, V: Visitor<'tree>>(v: &mut V, decl: &'tree ExternalDecl) {
    match decl {
        ExternalDecl::Precision(p) => {
            if matches!(v.visit_precision_decl(p, Visit::Pre), Walk::Skip) {
                return;
            }
            v.visit_precision_decl(p, Visit::Post);
        },
        ExternalDecl::Global(g) => {
            if matches!(v.visit_global_decl(g, Visit::Pre), Walk::Skip) {
                return;
            }
            v.visit_global_decl(g, Visit::Post);
        },
        ExternalDecl::Function(f) => walk_function_def(v, f),
        ExternalDecl::Struct(s) => {
            if matches!(v.visit_struct_decl(s, Visit::Pre), Walk::Skip) {
                return;
            }
            v.visit_struct_decl(s, Visit::Post);
        },
    }
}

pub fn walk_function_def<'tree, V: Visitor<'tree>>(v: &mut V, f: &'tree FunctionDef) {
    if matches!(v.visit_function_def(f, Visit::Pre), Walk::Skip) {
        return;
    }
    walk_block(v, &f.body);
    v.visit_function_def(f, Visit::Post);
}

pub fn walk_block<'tree, V: Visitor<'tree>>(v: &mut V, block: &'tree Block) {
    if matches!(v.visit_block(block, Visit::Pre), Walk::Skip) {
        return;
    }
    let n = block.stmts.len();
    for (i, stmt) in block.stmts.iter().enumerate() {
        walk_stmt(v, stmt);
        if i + 1 < n {
            v.visit_block(block, Visit::In);
        }
    }
    v.visit_block(block, Visit::Post);
}

pub fn walk_stmt<'tree, V: Visitor<'tree>>(v: &mut V, stmt: &'tree Stmt) {
    if matches!(v.visit_stmt(stmt, Visit::Pre), Walk::Skip) {
        return;
    }
    match stmt {
        Stmt::Expr(e) => walk_expr(v, e),
        Stmt::Return { value: Some(e), .. } => walk_expr(v, e),
        Stmt::Return { value: None, .. } => {},
        Stmt::Decl(d) => walk_local_decl(v, d),
        Stmt::Block(b) => walk_block(v, b),
        Stmt::If {
            cond, then, else_, ..
        } => {
            walk_expr(v, cond);
            v.visit_stmt(stmt, Visit::In);
            walk_stmt(v, then);
            if let Some(e) = else_ {
                v.visit_stmt(stmt, Visit::In);
                walk_stmt(v, e);
            }
        },
        Stmt::While { cond, body, .. } => {
            walk_expr(v, cond);
            v.visit_stmt(stmt, Visit::In);
            walk_stmt(v, body);
        },
        Stmt::Do { body, cond, .. } => {
            walk_stmt(v, body);
            v.visit_stmt(stmt, Visit::In);
            walk_expr(v, cond);
        },
        Stmt::For {
            init,
            cond,
            step,
            body,
            ..
        } => {
            match init {
                ForInit::Empty => {},
                ForInit::Decl(d) => walk_local_decl(v, d),
                ForInit::Expr(e) => walk_expr(v, e),
            }
            if let Some(c) = cond {
                walk_expr(v, c);
            }
            if let Some(s) = step {
                walk_expr(v, s);
            }
            walk_stmt(v, body);
        },
        Stmt::Break { .. } | Stmt::Continue { .. } | Stmt::Discard { .. } => {},
        Stmt::Switch {
            discriminant, body, ..
        } => {
            walk_expr(v, discriminant);
            v.visit_stmt(stmt, Visit::In);
            walk_block(v, body);
        },
        Stmt::Case { value, .. } => walk_expr(v, value),
        Stmt::Default { .. } => {},
    }
    v.visit_stmt(stmt, Visit::Post);
}

pub fn walk_local_decl<'tree, V: Visitor<'tree>>(v: &mut V, decl: &'tree LocalDecl) {
    if matches!(v.visit_local_decl(decl, Visit::Pre), Walk::Skip) {
        return;
    }
    if let Some(init) = &decl.init {
        walk_expr(v, init);
    }
    v.visit_local_decl(decl, Visit::Post);
}

pub fn walk_expr<'tree, V: Visitor<'tree>>(v: &mut V, expr: &'tree Expr) {
    if matches!(v.visit_expr(expr, Visit::Pre), Walk::Skip) {
        return;
    }
    match expr {
        Expr::IntLit { .. } | Expr::FloatLit { .. } | Expr::BoolLit { .. } | Expr::Ident { .. } => {
        },
        Expr::Call { args, .. } => {
            let n = args.len();
            for (i, a) in args.iter().enumerate() {
                walk_expr(v, a);
                if i + 1 < n {
                    v.visit_expr(expr, Visit::In);
                }
            }
        },
        Expr::Assign { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            walk_expr(v, lhs);
            v.visit_expr(expr, Visit::In);
            walk_expr(v, rhs);
        },
        Expr::Unary { expr: inner, .. } => walk_expr(v, inner),
        Expr::Member { base, .. } => walk_expr(v, base),
        Expr::Index { base, index, .. } => {
            walk_expr(v, base);
            v.visit_expr(expr, Visit::In);
            walk_expr(v, index);
        },
        Expr::Ternary {
            cond, then, else_, ..
        } => {
            walk_expr(v, cond);
            v.visit_expr(expr, Visit::In);
            walk_expr(v, then);
            v.visit_expr(expr, Visit::In);
            walk_expr(v, else_);
        },
    }
    v.visit_expr(expr, Visit::Post);
}
