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
    Expr, ExternalDecl, ForInit, FunctionDef, Stmt, TranslationUnit, TypeKind,
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
        }
    }
}

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
            },
            Visit::Post => {
                self.current_function = None;
            },
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_expr(&mut self, e: &'tree Expr, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            if let Expr::Call { callee, .. } = e {
                if let Some(caller) = self.current_function {
                    self.call_graph
                        .entry(caller.to_string())
                        .or_default()
                        .insert(callee.clone());
                }
            }
        }
        Walk::Continue
    }

    fn visit_stmt(&mut self, s: &'tree Stmt, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            if let Stmt::Discard { span } = s {
                if let Some(fname) = self.current_function {
                    self.discard_sites.push(DiscardSite { span: *span, function: fname });
                }
            }
        }
        // `for` init can be a local decl; the walker already routes
        // walk_local_decl through visit_local_decl, so no extra work
        // here. Leaving the match exhaustive-ish via fall-through.
        let _ = ForInit::Empty;
        Walk::Continue
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
