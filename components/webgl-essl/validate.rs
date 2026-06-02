/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebGL validator layer above the typecheck pass.
//!
//! Where [`crate::check`] enforces ESSL semantic rules, this module
//! enforces the *WebGL* delta: spec restrictions that make valid ESSL
//! invalid WebGL ESSL, plus the diagnostic shape WebGL implementations
//! return through `getShaderInfoLog`.
//!
//! Borrowed from ANGLE's `ParseContext` + `CallDAG` + `Diagnostics`
//! (cargo registry path
//! `mozangle-0.5.5/gfx/angle/checkout/src/compiler/translator/`). Lifted:
//!
//! * The `CallDAG` pattern (build a call graph, fail with a
//!   "recursion" verdict if it is not a DAG).
//! * `Diagnostics`'s severity-aware aggregation with separate error /
//!   warning counts, rendered to a single info-log string.
//! * The `ERROR: 0:<line>: <message>` line shape implementations use
//!   for `getShaderInfoLog`.
//!
//! Not lifted: ANGLE's pool allocator, `TInfoSinkBase`, the C++
//! preprocessor split. Those are translator implementation details,
//! not protocol.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ast::{
    BinOp, Expr, ExternalDecl, ForInit, FunctionDef, GlobalDecl, LocalDecl, Stmt, TranslationUnit,
    TypeKind, UnaryOp,
};
use crate::span::{Span, line_column};
use crate::visit::{Visit, Visitor, Walk, walk_translation_unit};

/// Which pipeline stage this shader source is compiled against.
/// Required because several spec rules (discard, gl_Position write,
/// gl_FragColor write) gate differently per stage.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
}

/// Run the validator over a parsed translation unit, producing the
/// diagnostic shape WebGL implementations return through
/// `getShaderInfoLog`.
pub fn validate(tu: &TranslationUnit, stage: ShaderStage) -> ValidationResult {
    let mut v = ValidatorVisitor::new(stage);
    walk_translation_unit(&mut v, tu);
    v.finalize(tu)
}

#[derive(Debug, Default)]
pub struct ValidationResult {
    pub errors: Vec<WebGlDiagnostic>,
    pub warnings: Vec<WebGlDiagnostic>,
    /// `getShaderInfoLog`-shaped text. Empty when there are no errors
    /// or warnings.
    pub info_log: String,
}

impl ValidationResult {
    pub fn num_errors(&self) -> usize {
        self.errors.len()
    }
    pub fn num_warnings(&self) -> usize {
        self.warnings.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebGlDiagnostic {
    pub severity: Severity,
    pub kind: WebGlDiagnosticKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebGlDiagnosticKind {
    /// User function is part of a recursive cycle. ESSL 1.00 §6.1
    /// "Recursion is not allowed."
    Recursion { cycle: Vec<String> },
    /// `discard` used outside a fragment shader. ESSL 1.00 §6.4
    /// reserves discard for fragment shaders.
    DiscardOutsideFragment { function: String },
    /// No `main` function defined; every shader must have one.
    MainNotDefined,
    /// `main` is defined but with the wrong signature; spec mandates
    /// `void main()` (or `void main(void)`).
    MainBadSignature { return_ty: TypeKind, param_count: usize },
    /// `for` loop does not match the Appendix A restricted form
    /// (ESSL 1.00 Appendix A §4): a counter-style loop with
    /// integer / float init, comparison cond against a constant, and
    /// loop-var increment / decrement step.
    ForLoopAppendixA { what: &'static str },
    /// User declaration of an identifier reserved for the
    /// implementation. Names beginning with `gl_`, `webgl_`,
    /// `_webgl_`, or containing `__` are reserved (ESSL 1.00 §3.7).
    ReservedIdentifier { name: String, reason: &'static str },
    /// Expression has more AST nodes than the per-expression complexity
    /// cap allows. Mirrors ANGLE's `limitExpressionComplexity`
    /// `CompileOptions` flag; the default cap is hardcoded today.
    ExpressionTooComplex { count: usize, limit: usize },
    /// Call chain reachable from a user function exceeds the call
    /// stack depth cap. Mirrors ANGLE's `limitCallStackDepth` flag.
    CallStackTooDeep { depth: usize, limit: usize },
}

impl WebGlDiagnosticKind {
    fn message(&self) -> String {
        match self {
            WebGlDiagnosticKind::Recursion { cycle } => {
                format!("recursion not allowed: {}", cycle.join(" -> "))
            },
            WebGlDiagnosticKind::DiscardOutsideFragment { function } => {
                format!("`discard` is only allowed in fragment shaders (in `{function}`)")
            },
            WebGlDiagnosticKind::MainNotDefined => "no `main` function defined".to_string(),
            WebGlDiagnosticKind::MainBadSignature { return_ty, param_count } => {
                format!(
                    "`main` must be `void main()`; got return type {return_ty:?} and {param_count} parameter(s)"
                )
            },
            WebGlDiagnosticKind::ForLoopAppendixA { what } => {
                format!("`for` loop does not match Appendix A restricted form: {what}")
            },
            WebGlDiagnosticKind::ReservedIdentifier { name, reason } => {
                format!("identifier `{name}` is reserved ({reason})")
            },
            WebGlDiagnosticKind::ExpressionTooComplex { count, limit } => {
                format!("expression too complex: {count} nodes (limit {limit})")
            },
            WebGlDiagnosticKind::CallStackTooDeep { depth, limit } => {
                format!("call stack too deep: {depth} (limit {limit})")
            },
        }
    }
}

/// Per-expression AST node count cap (matches ANGLE's default).
const MAX_EXPR_COMPLEXITY: usize = 256;
/// Maximum reachable call-chain depth from any user function
/// (matches ANGLE's default).
const MAX_CALL_STACK_DEPTH: usize = 16;

impl WebGlDiagnostic {
    /// Render in the standard `ERROR: 0:<line>: <message>` shape WebGL
    /// implementations use for `getShaderInfoLog`. Line is 1-based;
    /// column is omitted to match the dominant WebGL implementation
    /// (Chrome/ANGLE) format, since web authors mostly care about the
    /// line number.
    pub fn render_log_line(&self, src: &str) -> String {
        let (severity, _) = match self.severity {
            Severity::Error => ("ERROR", true),
            Severity::Warning => ("WARNING", false),
        };
        let line = self.span.map(|s| line_column(src, s.start).0).unwrap_or(0);
        format!("{severity}: 0:{line}: {message}", message = self.kind.message())
    }
}

impl fmt::Display for WebGlDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
        };
        write!(f, "{severity}: {message}", message = self.kind.message())
    }
}

// ---------- ValidatorVisitor ------------------------------------------

struct ValidatorVisitor<'tree> {
    stage: ShaderStage,
    current_function: Option<&'tree str>,
    /// `caller -> {callees}`. Built-ins reach here too; cycle detection
    /// only considers edges where the target is also a key (i.e., a
    /// user function defined in this translation unit).
    call_graph: HashMap<String, HashSet<String>>,
    discard_sites: Vec<DiscardSite<'tree>>,
    main_info: Option<MainInfo>,
    /// (span, list of violation descriptions). Each `for` loop produces
    /// at most one entry; the description is the first thing it failed
    /// to match, to match ANGLE's halt-on-first-issue diagnostic style.
    for_loop_violations: Vec<(Span, &'static str)>,
    /// (span where declared, name, reason it is reserved).
    reserved_identifiers: Vec<(Span, String, &'static str)>,
    /// Recursion depth inside `visit_expr`. When this is 0 on Pre, we
    /// know we are looking at a top-level expression (statement-expr,
    /// decl init, control-flow cond / step, function call arg from a
    /// statement, ...) and can count its full node tree against the
    /// per-expression cap.
    expr_depth: usize,
    /// (top-level expr span, total AST node count) for any expression
    /// exceeding the per-expression complexity cap.
    expr_too_complex: Vec<(Span, usize)>,
}

struct DiscardSite<'tree> {
    span: Span,
    function: &'tree str,
}

struct MainInfo {
    return_ty: TypeKind,
    param_count: usize,
}

impl<'tree> ValidatorVisitor<'tree> {
    fn new(stage: ShaderStage) -> Self {
        Self {
            stage,
            current_function: None,
            call_graph: HashMap::new(),
            discard_sites: Vec::new(),
            main_info: None,
            for_loop_violations: Vec::new(),
            reserved_identifiers: Vec::new(),
            expr_depth: 0,
            expr_too_complex: Vec::new(),
        }
    }

    fn note_reserved_if_any(&mut self, name: &str, span: Span) {
        if let Some(reason) = reserved_reason(name) {
            self.reserved_identifiers.push((span, name.to_string(), reason));
        }
    }

    fn finalize(self, src: &TranslationUnit) -> ValidationResult {
        let mut result = ValidationResult::default();

        // R1: No recursion. Restrict the call graph to user-defined
        // functions (built-ins can't cycle), then DFS for back edges.
        let user_fns: HashSet<String> = src
            .decls
            .iter()
            .filter_map(|d| match d {
                ExternalDecl::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect();
        let cycles = find_cycles(&self.call_graph, &user_fns);
        for cycle in cycles {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::Recursion { cycle },
                span: None,
            });
        }

        // R2: Discard only in fragment shaders.
        if self.stage == ShaderStage::Vertex {
            for site in self.discard_sites {
                result.errors.push(WebGlDiagnostic {
                    severity: Severity::Error,
                    kind: WebGlDiagnosticKind::DiscardOutsideFragment {
                        function: site.function.to_string(),
                    },
                    span: Some(site.span),
                });
            }
        }

        // R3: main signature.
        match self.main_info {
            None => result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::MainNotDefined,
                span: None,
            }),
            Some(info) => {
                if info.return_ty != TypeKind::Void || info.param_count != 0 {
                    result.errors.push(WebGlDiagnostic {
                        severity: Severity::Error,
                        kind: WebGlDiagnosticKind::MainBadSignature {
                            return_ty: info.return_ty,
                            param_count: info.param_count,
                        },
                        span: None,
                    });
                }
            },
        }

        // R4: Appendix A `for` loop restriction.
        for (span, what) in self.for_loop_violations {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ForLoopAppendixA { what },
                span: Some(span),
            });
        }

        // R5: Reserved identifier prefixes.
        for (span, name, reason) in self.reserved_identifiers {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ReservedIdentifier { name, reason },
                span: Some(span),
            });
        }

        // R6: Per-expression complexity cap.
        for (span, count) in self.expr_too_complex {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ExpressionTooComplex {
                    count,
                    limit: MAX_EXPR_COMPLEXITY,
                },
                span: Some(span),
            });
        }

        // R7: Call stack depth cap. Computed on the user-function
        // subgraph. Only meaningful when no R1 cycle was detected
        // (cycles produce infinite depth, but R1 already flagged them).
        if cycles_were_empty(&result) {
            let depth = longest_user_call_depth(&self.call_graph, &user_fns);
            if depth > MAX_CALL_STACK_DEPTH {
                result.errors.push(WebGlDiagnostic {
                    severity: Severity::Error,
                    kind: WebGlDiagnosticKind::CallStackTooDeep {
                        depth,
                        limit: MAX_CALL_STACK_DEPTH,
                    },
                    span: None,
                });
            }
        }

        // Render info_log. ANGLE / Chrome shape: one line per
        // error / warning, errors first, then warnings.
        let source_text = "";
        let mut lines: Vec<String> = result
            .errors
            .iter()
            .chain(result.warnings.iter())
            .map(|d| d.render_log_line(source_text))
            .collect();
        if !lines.is_empty() {
            lines.push(String::new());
        }
        result.info_log = lines.join("\n");
        result
    }
}

impl<'tree> Visitor<'tree> for ValidatorVisitor<'tree> {
    fn visit_function_def(&mut self, fd: &'tree FunctionDef, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                self.current_function = Some(fd.name.as_str());
                if fd.name == "main" {
                    self.main_info = Some(MainInfo {
                        return_ty: fd.return_ty.kind,
                        param_count: fd.params.len(),
                    });
                }
                self.call_graph.entry(fd.name.clone()).or_default();
                self.note_reserved_if_any(&fd.name, fd.name_span);
                for p in &fd.params {
                    self.note_reserved_if_any(&p.name, p.span);
                }
            },
            Visit::Post => {
                self.current_function = None;
            },
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_global_decl(&mut self, g: &'tree GlobalDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            self.note_reserved_if_any(&g.name, g.name_span);
        }
        Walk::Continue
    }

    fn visit_local_decl(&mut self, d: &'tree LocalDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            self.note_reserved_if_any(&d.name, d.name_span);
        }
        Walk::Continue
    }

    fn visit_expr(&mut self, e: &'tree Expr, visit: Visit) -> Walk {
        match visit {
            Visit::Pre => {
                // Top-level expression: count its full AST node tree
                // against the per-expression complexity cap before
                // descending.
                if self.expr_depth == 0 {
                    let count = count_expr_nodes(e);
                    if count > MAX_EXPR_COMPLEXITY {
                        self.expr_too_complex.push((e.span(), count));
                    }
                }
                self.expr_depth += 1;
                if let Expr::Call { callee, .. } = e {
                    if let Some(caller) = self.current_function {
                        self.call_graph
                            .entry(caller.to_string())
                            .or_default()
                            .insert(callee.clone());
                    }
                }
            },
            Visit::Post => {
                self.expr_depth -= 1;
            },
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_stmt(&mut self, s: &'tree Stmt, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            match s {
                Stmt::Discard { span } => {
                    if let Some(fname) = self.current_function {
                        self.discard_sites.push(DiscardSite { span: *span, function: fname });
                    }
                },
                Stmt::For { span, .. } => {
                    if let Some(what) = check_for_appendix_a(s) {
                        self.for_loop_violations.push((*span, what));
                    }
                },
                _ => {},
            }
        }
        Walk::Continue
    }
}

// ---------- R4: Appendix A `for` loop check ---------------------------
//
// ESSL 1.00 Appendix A §4 restricts `for` loops to a counter-style
// form. The full rule covers init, cond, step, and forbids the body
// from modifying the loop variable. The body-modification check is a
// deeper analysis and is queued for a follow-up; today's check covers
// init / cond / step shape, which catches the bulk of real-world
// hostile-input patterns.

fn check_for_appendix_a(stmt: &Stmt) -> Option<&'static str> {
    let Stmt::For { init, cond, step, .. } = stmt else {
        return None;
    };

    // Init: must declare a single integer or float loop variable with
    // an initializer.
    let loop_var: &str = match init {
        ForInit::Decl(d) => {
            if !matches!(d.ty.kind, TypeKind::Int | TypeKind::Float) {
                return Some("loop variable must be `int` or `float`");
            }
            if d.init.is_none() {
                return Some("loop variable must have an initializer");
            }
            d.name.as_str()
        },
        ForInit::Empty => return Some("missing loop variable declaration"),
        ForInit::Expr(_) => {
            return Some("loop variable must be declared inside the for-init");
        },
    };

    // Cond: must be a comparison binary expression involving the loop
    // var.
    match cond {
        None => return Some("missing loop condition"),
        Some(Expr::Binary { op, lhs, rhs, .. }) => {
            let is_comparison = matches!(
                op,
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
            );
            if !is_comparison {
                return Some(
                    "loop condition operator must be `<`, `<=`, `>`, `>=`, `==`, or `!=`",
                );
            }
            let lhs_is_var = matches!(
                lhs.as_ref(),
                Expr::Ident { name, .. } if name == loop_var
            );
            let rhs_is_var = matches!(
                rhs.as_ref(),
                Expr::Ident { name, .. } if name == loop_var
            );
            if !lhs_is_var && !rhs_is_var {
                return Some("loop condition must reference the loop variable");
            }
        },
        Some(_) => return Some("loop condition must be a comparison expression"),
    }

    // Step: must update the loop variable via ++ / -- / += / -= / *= / /=.
    match step {
        None => return Some("missing loop step expression"),
        Some(e) => {
            let updates_loop_var = match e {
                Expr::Unary { op, expr, .. } => {
                    matches!(
                        op,
                        UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec
                    ) && matches!(
                        expr.as_ref(),
                        Expr::Ident { name, .. } if name == loop_var
                    )
                },
                Expr::Assign { lhs, .. } => matches!(
                    lhs.as_ref(),
                    Expr::Ident { name, .. } if name == loop_var
                ),
                _ => false,
            };
            if !updates_loop_var {
                return Some("loop step must update the loop variable");
            }
        },
    }

    None
}

// ---------- R6: Per-expression AST node count -------------------------

fn count_expr_nodes(e: &Expr) -> usize {
    1 + match e {
        Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::BoolLit { .. }
        | Expr::Ident { .. } => 0,
        Expr::Binary { lhs, rhs, .. } | Expr::Assign { lhs, rhs, .. } => {
            count_expr_nodes(lhs) + count_expr_nodes(rhs)
        },
        Expr::Unary { expr, .. } => count_expr_nodes(expr),
        Expr::Member { base, .. } => count_expr_nodes(base),
        Expr::Index { base, index, .. } => count_expr_nodes(base) + count_expr_nodes(index),
        Expr::Ternary { cond, then, else_, .. } => {
            count_expr_nodes(cond) + count_expr_nodes(then) + count_expr_nodes(else_)
        },
        Expr::Call { args, .. } => args.iter().map(count_expr_nodes).sum::<usize>(),
    }
}

// ---------- R7: Longest call chain over user functions -----------------

fn cycles_were_empty(r: &ValidationResult) -> bool {
    !r.errors
        .iter()
        .any(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. }))
}

fn longest_user_call_depth(
    graph: &HashMap<String, HashSet<String>>,
    user_fns: &HashSet<String>,
) -> usize {
    let mut memo: HashMap<String, usize> = HashMap::new();
    let mut max_depth = 0;
    for n in user_fns {
        let d = depth_of(n, graph, user_fns, &mut memo);
        max_depth = max_depth.max(d);
    }
    max_depth
}

fn depth_of(
    node: &str,
    graph: &HashMap<String, HashSet<String>>,
    user_fns: &HashSet<String>,
    memo: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&d) = memo.get(node) {
        return d;
    }
    let mut best_child = 0usize;
    if let Some(callees) = graph.get(node) {
        let snapshot: Vec<String> = callees.iter().cloned().collect();
        for c in snapshot {
            if user_fns.contains(&c) {
                let d = depth_of(&c, graph, user_fns, memo);
                best_child = best_child.max(d);
            }
        }
    }
    let d = 1 + best_child;
    memo.insert(node.to_string(), d);
    d
}

// ---------- R5: Reserved identifier prefix check ----------------------

fn reserved_reason(name: &str) -> Option<&'static str> {
    if name.starts_with("gl_") {
        Some("starts with `gl_`")
    } else if name.starts_with("webgl_") {
        Some("starts with `webgl_`")
    } else if name.starts_with("_webgl_") {
        Some("starts with `_webgl_`")
    } else if name.contains("__") {
        Some("contains `__`")
    } else {
        None
    }
}

// ---------- cycle detection (CallDAG-shaped) --------------------------

#[derive(Clone, Copy, PartialEq)]
enum Color {
    White,
    Gray,
    Black,
}

fn find_cycles(
    graph: &HashMap<String, HashSet<String>>,
    user_fns: &HashSet<String>,
) -> Vec<Vec<String>> {
    let mut color: HashMap<String, Color> = HashMap::new();
    let mut cycles: Vec<Vec<String>> = Vec::new();
    let mut seen_cycle: HashSet<Vec<String>> = HashSet::new();
    let nodes: Vec<String> = user_fns.iter().cloned().collect();
    for start in &nodes {
        if color.get(start).copied().unwrap_or(Color::White) != Color::White {
            continue;
        }
        let mut path: Vec<String> = Vec::new();
        dfs(start, graph, user_fns, &mut color, &mut path, &mut cycles, &mut seen_cycle);
    }
    cycles
}

fn dfs(
    node: &str,
    graph: &HashMap<String, HashSet<String>>,
    user_fns: &HashSet<String>,
    color: &mut HashMap<String, Color>,
    path: &mut Vec<String>,
    cycles: &mut Vec<Vec<String>>,
    seen: &mut HashSet<Vec<String>>,
) {
    color.insert(node.to_string(), Color::Gray);
    path.push(node.to_string());
    if let Some(callees) = graph.get(node) {
        let callees: Vec<String> = callees.iter().cloned().collect();
        for callee in callees {
            if !user_fns.contains(&callee) {
                continue;
            }
            match color.get(&callee).copied().unwrap_or(Color::White) {
                Color::Gray => {
                    if let Some(from) = path.iter().position(|n| n == &callee) {
                        let mut cycle: Vec<String> = path[from..].to_vec();
                        cycle.push(callee.clone());
                        let canonical = canonicalize_cycle(&cycle);
                        if seen.insert(canonical) {
                            cycles.push(cycle);
                        }
                    }
                },
                Color::White => {
                    dfs(&callee, graph, user_fns, color, path, cycles, seen);
                },
                Color::Black => {},
            }
        }
    }
    path.pop();
    color.insert(node.to_string(), Color::Black);
}

fn canonicalize_cycle(cycle: &[String]) -> Vec<String> {
    // Drop the duplicate trailing node, rotate so the smallest name
    // leads. Lets `[f, g, f]` and `[g, f, g]` collapse to the same key.
    if cycle.len() < 2 {
        return cycle.to_vec();
    }
    let mut body: Vec<String> = cycle[..cycle.len() - 1].to_vec();
    let min_pos = body
        .iter()
        .enumerate()
        .min_by_key(|(_, n)| n.as_str())
        .map(|(i, _)| i)
        .unwrap_or(0);
    body.rotate_left(min_pos);
    body
}
