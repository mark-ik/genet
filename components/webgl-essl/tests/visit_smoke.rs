/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Smoke for the visitor infrastructure: a `NodeCounter` that records
//! Pre-phase visits per node kind, plus a `Skipper` proving
//! `Walk::Skip` from a PreVisit prunes the subtree but lets siblings
//! continue. The point is to exercise the walker mechanics; the
//! per-kind counts and the skip semantics are what every typecheck or
//! validator pass will lean on.

use webgl_essl::ast::*;
use webgl_essl::parse_source;
use webgl_essl::visit::{Visit, Visitor, Walk, walk_translation_unit};

#[derive(Default)]
struct NodeCounter {
    translation_unit: usize,
    precision_decl: usize,
    global_decl: usize,
    function_def: usize,
    struct_decl: usize,
    block: usize,
    stmt: usize,
    local_decl: usize,
    expr: usize,
}

impl<'tree> Visitor<'tree> for NodeCounter {
    fn visit_translation_unit(&mut self, _: &'tree TranslationUnit, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.translation_unit += 1;
        }
        Walk::Continue
    }
    fn visit_precision_decl(&mut self, _: &'tree PrecisionDecl, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.precision_decl += 1;
        }
        Walk::Continue
    }
    fn visit_global_decl(&mut self, _: &'tree GlobalDecl, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.global_decl += 1;
        }
        Walk::Continue
    }
    fn visit_function_def(&mut self, _: &'tree FunctionDef, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.function_def += 1;
        }
        Walk::Continue
    }
    fn visit_struct_decl(&mut self, _: &'tree StructDecl, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.struct_decl += 1;
        }
        Walk::Continue
    }
    fn visit_block(&mut self, _: &'tree Block, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.block += 1;
        }
        Walk::Continue
    }
    fn visit_stmt(&mut self, _: &'tree Stmt, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.stmt += 1;
        }
        Walk::Continue
    }
    fn visit_local_decl(&mut self, _: &'tree LocalDecl, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.local_decl += 1;
        }
        Walk::Continue
    }
    fn visit_expr(&mut self, _: &'tree Expr, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.expr += 1;
        }
        Walk::Continue
    }
}

fn count(src: &str) -> NodeCounter {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let mut counter = NodeCounter::default();
    walk_translation_unit(&mut counter, &tu);
    counter
}

#[test]
fn solid_color_shader_counts_match_hand_audit() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let c = count(src);
    assert_eq!(c.translation_unit, 1);
    assert_eq!(c.precision_decl, 1);
    assert_eq!(c.global_decl, 1, "uniform decl");
    assert_eq!(c.function_def, 1, "main()");
    assert_eq!(c.struct_decl, 0);
    assert_eq!(c.block, 1, "main body");
    assert_eq!(c.stmt, 1, "the assignment statement");
    assert_eq!(c.local_decl, 0);
    // 3 exprs: the assignment node, plus its lhs ident and rhs ident.
    assert_eq!(c.expr, 3, "assign + lhs ident + rhs ident");
}

#[test]
fn counts_scale_with_a_more_complex_shader() {
    // Vertex + a few attributes / varyings / locals / control flow.
    let src = r#"
attribute vec2 a_position;
attribute vec3 a_color;
varying vec3 v_color;
uniform float u_scale;
void main() {
    vec2 scaled = a_position * u_scale;
    if (u_scale > 0.0) {
        v_color = a_color;
    } else {
        v_color = vec3(0.0);
    }
    gl_Position = vec4(scaled, 0.0, 1.0);
}
"#;
    let c = count(src);
    assert_eq!(c.translation_unit, 1);
    assert_eq!(c.precision_decl, 0);
    assert_eq!(c.global_decl, 4, "a_position, a_color, v_color, u_scale");
    assert_eq!(c.function_def, 1);
    assert_eq!(c.block, 3, "main body + then-branch block + else-branch block");
    // 7 visit_stmt calls: local decl `scaled`, the `if`, the `then` Stmt::Block
    // wrapper, the assign inside it, the `else` Stmt::Block wrapper, the
    // assign inside it, the `gl_Position` assign. `Stmt::Block(b)` is a
    // Stmt variant so visit_stmt fires for the wrapper before walk_block
    // descends into the contained Block.
    assert_eq!(c.stmt, 7);
    assert_eq!(c.local_decl, 1);
    // Exprs are harder to hand-audit precisely; assert sanity bounds.
    assert!(c.expr >= 15, "got {} (expected many)", c.expr);
}

// ---------- Skip semantics ---------------------------------------------

/// Visitor that returns `Walk::Skip` for one specific external-decl
/// kind during PreVisit. Counts how many exprs were visited; the skip
/// should prevent descent into the skipped subtree while leaving the
/// other external decls intact.
#[derive(Default)]
struct Skipper {
    expr_count: usize,
    function_def_count: usize,
}

impl<'tree> Visitor<'tree> for Skipper {
    fn visit_function_def(&mut self, _: &'tree FunctionDef, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.function_def_count += 1;
            return Walk::Skip;
        }
        Walk::Continue
    }
    fn visit_expr(&mut self, _: &'tree Expr, v: Visit) -> Walk {
        if v == Visit::Pre {
            self.expr_count += 1;
        }
        Walk::Continue
    }
}

#[test]
fn skip_from_pre_visit_prunes_the_function_body() {
    let src = r#"
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let tu = parse_source(src).expect("parse");
    let mut s = Skipper::default();
    walk_translation_unit(&mut s, &tu);
    assert_eq!(s.function_def_count, 1, "PreVisit fires");
    // Skipping the function def's PreVisit means walk_block is never
    // entered, so no exprs from gl_FragColor = u_color get counted.
    assert_eq!(s.expr_count, 0, "function body skipped entirely");
}

// ---------- Path tracking pattern (sanity sketch) ----------------------

/// Sketch of the path-tracking pattern the typecheck pass will use:
/// the visitor maintains its own ancestor stack via Pre / Post. Tests
/// that the stack returns to empty after a full walk.
#[derive(Default)]
struct PathTracker<'tree> {
    function_stack: Vec<&'tree FunctionDef>,
    saw_function_in_path: bool,
    deepest_function_stack: usize,
}

impl<'tree> Visitor<'tree> for PathTracker<'tree> {
    fn visit_function_def(&mut self, node: &'tree FunctionDef, v: Visit) -> Walk {
        match v {
            Visit::Pre => {
                self.function_stack.push(node);
                self.deepest_function_stack =
                    self.deepest_function_stack.max(self.function_stack.len());
            },
            Visit::Post => {
                self.function_stack.pop();
            },
            Visit::In => {},
        }
        Walk::Continue
    }
    fn visit_expr(&mut self, _: &'tree Expr, v: Visit) -> Walk {
        if v == Visit::Pre && !self.function_stack.is_empty() {
            self.saw_function_in_path = true;
        }
        Walk::Continue
    }
}

#[test]
fn path_tracker_pushes_and_pops_around_function_def() {
    let src = r#"
float helper(float x) { return x + 1.0; }
void main() { gl_FragColor = vec4(helper(0.5)); }
"#;
    let tu = parse_source(src).expect("parse");
    let mut p = PathTracker::default();
    walk_translation_unit(&mut p, &tu);
    assert!(p.function_stack.is_empty(), "stack returns to empty");
    assert_eq!(p.deepest_function_stack, 1, "no nested functions in ESSL");
    assert!(p.saw_function_in_path, "exprs visited under a function ancestor");
}
