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
    BinOp, Expr, ExternalDecl, ForInit, FunctionDef, GlobalDecl, LocalDecl, PrecisionDecl, Stmt,
    TranslationUnit, TypeKind, UnaryOp,
};

/// WebGL 1 §6.4 minimum-guaranteed packing limits. Real
/// implementations may support more; these are the floors a shader
/// must respect to be portable.
const MAX_VERTEX_ATTRIBS: u32 = 8;
const MAX_VARYING_VECTORS: u32 = 8;
const MAX_VERTEX_UNIFORM_VECTORS: u32 = 128;
const MAX_FRAGMENT_UNIFORM_VECTORS: u32 = 16;

/// Slot count for a single declaration of the given type, with no
/// inter-decl packing. Conservative: scalars and vec_n cost one vec4
/// slot each, matrices cost their column count, samplers and void
/// don't count toward vector limits. R13 sums this over all decls in
/// each storage class.
fn slot_count_for(ty: TypeKind) -> u32 {
    match ty {
        TypeKind::Bool | TypeKind::Int | TypeKind::Float => 1,
        TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4 => 1,
        TypeKind::Bvec2 | TypeKind::Bvec3 | TypeKind::Bvec4 => 1,
        TypeKind::Ivec2 | TypeKind::Ivec3 | TypeKind::Ivec4 => 1,
        TypeKind::Mat2 => 2,
        TypeKind::Mat3 => 3,
        TypeKind::Mat4 => 4,
        TypeKind::Void | TypeKind::Sampler2D | TypeKind::SamplerCube => 0,
    }
}
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
///
/// `source` is the original ESSL text the translation unit was parsed
/// from; it is consulted only to resolve diagnostic spans into 1-based
/// line numbers for the `ERROR: 0:<line>: ...` info-log shape. Passing
/// an empty string is valid and produces lines with `0:0:` line markers
/// (the previous behavior before this seam was widened).
///
/// Internally calls [`crate::check::check`] to get per-span type
/// annotations; rules R9 / R10 use them to gate switch-discriminant
/// and case-value typing.
pub fn validate(tu: &TranslationUnit, source: &str, stage: ShaderStage) -> ValidationResult {
    let check_result = crate::check::check(tu);
    let mut v = ValidatorVisitor::new(stage, check_result.types, tu.version);
    walk_translation_unit(&mut v, tu);
    v.finalize(tu, source)
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
    /// Fragment-stage declaration of a float-family type with no
    /// precision qualifier and no preceding `precision <q> float;`
    /// default. ESSL 1.00 §4.5.3: "The fragment language has no
    /// default precision qualifier for floating point types."
    PrecisionMissingForFloat { name: String, ty: TypeKind },
    /// `switch` discriminant did not resolve to an integer type
    /// (ESSL 3.00 §6.5: "The init-expression must be of type int").
    SwitchDiscriminantNotInt { actual: TypeKind },
    /// `case <value>:` label's value is not a literal integer
    /// constant (ESSL 3.00 §6.5: case labels must be integer
    /// constant expressions). The first-pass implementation only
    /// accepts literal `IntLit`s; full constant-folding is queued.
    CaseValueNotIntegerConstant,
    /// Two `case` labels inside the same switch share the same
    /// integer value.
    DuplicateCaseValue { value: i64 },
    /// `arr[<expr>]` where `<expr>` is neither a literal integer
    /// constant nor an active loop induction variable. ESSL 1.00
    /// Appendix A restricts WebGL 1 array indexing to constant-
    /// index-expressions. ESSL 3.00 relaxes this; the rule is
    /// gated to ESSL 1.00 source.
    IndirectArrayIndex,
    /// `const T x;` declared without an initializer.
    /// ESSL §4.3 requires `const` to have an init.
    ConstWithoutInit { name: String },
    /// `const T x = <expr>;` where `<expr>` is not a constant
    /// expression per the first-pass acceptance set (literal +
    /// recursive unary/binary on constants).
    ConstInitNotConstant { name: String },
    /// Assignment whose LHS targets a `const`-qualified local.
    ConstAssignment { name: String },
    /// A non-void user function's body does not end with a
    /// `return <expr>;`. ESSL §6.4: every path through a non-void
    /// function must return a value.
    MissingReturnInNonVoidFunction { name: String },
    /// `return <expr>;` whose expression type does not match the
    /// declared return type. ESSL §6.1 forbids implicit conversions.
    ReturnTypeMismatch { expected: TypeKind, actual: TypeKind },
    /// Two user function definitions share the same name AND the
    /// same parameter types. ESSL §6.1.1.
    FunctionRedefinition { name: String },
    /// `attribute T x;` declared in a fragment shader. ESSL §4.3.3
    /// restricts `attribute` to the vertex stage.
    AttributeInFragmentShader { name: String },
    /// A storage class exceeds its WebGL 1 minimum-guaranteed slot
    /// count. WebGL 1 spec §6.4 fixes:
    /// - attribute: 8 (MAX_VERTEX_ATTRIBS)
    /// - varying: 8 (MAX_VARYING_VECTORS)
    /// - vertex uniform: 128 (MAX_VERTEX_UNIFORM_VECTORS)
    /// - fragment uniform: 16 (MAX_FRAGMENT_UNIFORM_VECTORS)
    /// The slot-count rule is one slot per declaration for
    /// scalars/vec_n, n slots for mat_n; samplers do not count.
    /// Conservative — does not yet implement Appendix A's
    /// inter-declaration packing scheme, so it rejects some shaders
    /// real implementations could pack.
    PackingLimitExceeded { class: &'static str, used: u32, limit: u32 },
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
            WebGlDiagnosticKind::PrecisionMissingForFloat { name, ty } => {
                format!(
                    "no precision specified for `{name}` ({ty:?}); fragment shaders require a precision qualifier or a default set via `precision <q> float;`"
                )
            },
            WebGlDiagnosticKind::SwitchDiscriminantNotInt { actual } => {
                format!("`switch` discriminant must be `int`, got {actual:?}")
            },
            WebGlDiagnosticKind::CaseValueNotIntegerConstant => {
                "`case <value>:` value must be a literal integer constant".to_string()
            },
            WebGlDiagnosticKind::DuplicateCaseValue { value } => {
                format!("duplicate `case {value}:` label within the same switch")
            },
            WebGlDiagnosticKind::IndirectArrayIndex => {
                "array index must be a constant or a loop induction variable (ESSL 1.00 Appendix A)".to_string()
            },
            WebGlDiagnosticKind::ConstWithoutInit { name } => {
                format!("`const {name}` declared without an initializer")
            },
            WebGlDiagnosticKind::ConstInitNotConstant { name } => {
                format!("initializer for `const {name}` is not a constant expression")
            },
            WebGlDiagnosticKind::ConstAssignment { name } => {
                format!("cannot assign to `const` variable `{name}`")
            },
            WebGlDiagnosticKind::MissingReturnInNonVoidFunction { name } => {
                format!("function `{name}` has a non-void return type but its body does not end with a `return`")
            },
            WebGlDiagnosticKind::ReturnTypeMismatch { expected, actual } => {
                format!("`return` expression has type {actual:?}, expected {expected:?}")
            },
            WebGlDiagnosticKind::FunctionRedefinition { name } => {
                format!("function `{name}` is redefined with the same parameter types")
            },
            WebGlDiagnosticKind::AttributeInFragmentShader { name } => {
                format!("`attribute {name}` declared in a fragment shader (attribute is vertex-only per ESSL §4.3.3)")
            },
            WebGlDiagnosticKind::PackingLimitExceeded { class, used, limit } => {
                format!("too many {class} slots: {used} declared, WebGL minimum is {limit}")
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
    /// True once a `precision <q> float;` declaration has been seen
    /// at file scope. Subsequent float-family declarations without
    /// inline precision are treated as having the default.
    float_default_set: bool,
    /// (span where declared, name, type) for fragment-stage decls of
    /// float-family types that have no inline precision and no
    /// preceding default. Only populated when stage == Fragment.
    precision_missing: Vec<(Span, String, TypeKind)>,
    /// Per-span type annotations from [`crate::check`]. Consulted by
    /// R9 to gate the switch discriminant type.
    types: HashMap<Span, TypeKind>,
    /// (switch-stmt span, discriminant type) for switches whose
    /// discriminant did not resolve to `int`.
    switch_discriminant_bad: Vec<(Span, TypeKind)>,
    /// (case-stmt span) for case labels whose value is not a literal
    /// integer constant. ESSL 1.00 §6.5 requires a constant integer
    /// expression.
    case_value_not_constant: Vec<Span>,
    /// (case-stmt span, duplicate value) for duplicate case values
    /// within the same switch. Stack-of-sets pattern: each switch
    /// body pushes a fresh set on visit_stmt Pre.
    case_duplicates: Vec<(Span, i64)>,
    /// Stack of sets of case values seen so far within each
    /// enclosing switch's body. Used to detect duplicates as the
    /// visitor descends into nested switches.
    switch_case_stack: Vec<HashSet<i64>>,
    /// `#version` directive value, propagated from
    /// [`TranslationUnit::version`]. Used by R12 to gate the
    /// indirect-array-index rule to ESSL 1.00. `None` is treated
    /// as ESSL 1.00 (the WebGL 1 default).
    version: Option<u32>,
    /// Stack of currently-active loop induction variable names.
    /// Pushed on Stmt::For Pre, popped on Stmt::For Post. Used by
    /// R12 to admit `arr[i]` when `i` is a loop iter var even
    /// though it is not a literal integer constant.
    loop_var_stack: Vec<String>,
    /// (index-expression span) for `arr[expr]` sites where the
    /// index is neither a constant nor a loop iter var. ESSL 1.00
    /// Appendix A constrains array indexing to constant-index-
    /// expressions; full WebGL conformance requires this.
    indirect_index_sites: Vec<Span>,
    /// (name span, name) for `const T x;` declarations missing
    /// an initializer. ESSL §4.3 requires `const` to be initialized.
    const_without_init_named: Vec<(Span, String)>,
    /// (name span, name) for `const T x = <expr>;` declarations
    /// whose initializer is not a constant expression.
    const_init_not_constant_named: Vec<(Span, String)>,
    /// Names of currently-in-scope `const` locals. Used by R15 to
    /// flag assignments that target a const binding.
    const_locals: HashSet<String>,
    /// (assignment-expression span, target name) for assignments
    /// whose LHS is a `const`-qualified local. ESSL §4.3 forbids
    /// modifying a `const` variable.
    const_assignment_sites: Vec<(Span, String)>,
    /// (function-def span, name) for non-void user functions whose
    /// body does not end with a `Stmt::Return`. ESSL §6.4 requires
    /// every path through a non-void function to return.
    missing_return_sites: Vec<(Span, String)>,
    /// Currently-lowering function's return type. Set on
    /// `visit_function_def` Pre, cleared on Post. R17 uses it to
    /// gate `return <expr>;` typing inside the body.
    current_return_ty: Option<TypeKind>,
    /// (return-stmt span, expected, actual) for `return <expr>;`
    /// whose expression type does not match the enclosing
    /// function's declared return type.
    return_type_mismatch_sites: Vec<(Span, TypeKind, TypeKind)>,
    /// Map name -> first-seen (param types, span) for user
    /// function definitions. R18 fires when a second definition
    /// with the same name and same param types arrives.
    function_signatures: HashMap<String, (Vec<TypeKind>, Span)>,
    /// (function-def span, name) for redefined user functions.
    function_redefinition_sites: Vec<(Span, String)>,
    /// (global-decl span, name) for `attribute T x;` declared in a
    /// fragment shader. ESSL §4.3.3 restricts `attribute` to
    /// vertex shaders only.
    attribute_in_fragment_sites: Vec<(Span, String)>,
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
    /// True if the source was parsed under ESSL 1.00 (the WebGL 1
    /// default). `None` is treated as 1.00 per the spec default;
    /// `Some(100)` is the explicit form. Any other version (300, ...)
    /// is ESSL 3.00+.
    fn is_essl_100(&self) -> bool {
        matches!(self.version, None | Some(100))
    }

    /// R12 first-pass acceptance set: literal int, an Ident that
    /// resolves to a currently-active loop induction variable, or a
    /// unary/binary expression formed recursively from those.
    /// Conservative — does not yet handle `const int N = ...; arr[N]`
    /// (constant folding queued).
    fn is_constant_index_expr(&self, e: &Expr) -> bool {
        match e {
            Expr::IntLit { .. } => true,
            Expr::Ident { name, .. } => self.loop_var_stack.iter().any(|v| v == name),
            Expr::Unary { expr, .. } => self.is_constant_index_expr(expr),
            Expr::Binary { lhs, rhs, .. } => {
                self.is_constant_index_expr(lhs) && self.is_constant_index_expr(rhs)
            },
            _ => false,
        }
    }

    fn new(stage: ShaderStage, types: HashMap<Span, TypeKind>, version: Option<u32>) -> Self {
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
            float_default_set: false,
            precision_missing: Vec::new(),
            types,
            switch_discriminant_bad: Vec::new(),
            case_value_not_constant: Vec::new(),
            case_duplicates: Vec::new(),
            switch_case_stack: Vec::new(),
            version,
            loop_var_stack: Vec::new(),
            indirect_index_sites: Vec::new(),
            const_without_init_named: Vec::new(),
            const_init_not_constant_named: Vec::new(),
            const_locals: HashSet::new(),
            const_assignment_sites: Vec::new(),
            missing_return_sites: Vec::new(),
            current_return_ty: None,
            return_type_mismatch_sites: Vec::new(),
            function_signatures: HashMap::new(),
            function_redefinition_sites: Vec::new(),
            attribute_in_fragment_sites: Vec::new(),
        }
    }

    fn note_reserved_if_any(&mut self, name: &str, span: Span) {
        if let Some(reason) = reserved_reason(name) {
            self.reserved_identifiers.push((span, name.to_string(), reason));
        }
    }

    fn note_precision_missing_if_any(
        &mut self,
        name: &str,
        span: Span,
        ty: TypeKind,
        has_inline_precision: bool,
    ) {
        if self.stage != ShaderStage::Fragment {
            return;
        }
        if !is_float_family(ty) {
            return;
        }
        if has_inline_precision || self.float_default_set {
            return;
        }
        self.precision_missing.push((span, name.to_string(), ty));
    }

    fn finalize(self, src: &TranslationUnit, source_text: &str) -> ValidationResult {
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

        // R8: Missing precision on float-family declarations in
        // fragment shaders. Only fires when the stage is Fragment;
        // vertex defaults float precision to highp.
        for (span, name, ty) in self.precision_missing {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::PrecisionMissingForFloat { name, ty },
                span: Some(span),
            });
        }

        // R9: switch discriminant must be int.
        for (span, actual) in self.switch_discriminant_bad {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::SwitchDiscriminantNotInt { actual },
                span: Some(span),
            });
        }

        // R10: case value must be an integer constant.
        for span in self.case_value_not_constant {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::CaseValueNotIntegerConstant,
                span: Some(span),
            });
        }

        // R11: duplicate case values within the same switch.
        for (span, value) in self.case_duplicates {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::DuplicateCaseValue { value },
                span: Some(span),
            });
        }

        // R12: indirect array index (ESSL 1.00 only).
        for span in self.indirect_index_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::IndirectArrayIndex,
                span: Some(span),
            });
        }

        // R14: `const T x;` requires an initializer; the
        // initializer must be a constant expression. First-pass
        // acceptance set: literal IntLit / FloatLit / BoolLit and
        // recursive unary / binary on constants.
        // The names are looked up by reconstructing them from the
        // span; for the diagnostic shape we pass back the bare
        // span text would require source. Simpler: store the name
        // alongside the span at note time. (Already done below.)
        for (span, name) in &self.const_without_init_named {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ConstWithoutInit { name: name.clone() },
                span: Some(*span),
            });
        }
        for (span, name) in &self.const_init_not_constant_named {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ConstInitNotConstant { name: name.clone() },
                span: Some(*span),
            });
        }

        // R15: assignment to a `const`-qualified local.
        for (span, name) in self.const_assignment_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ConstAssignment { name },
                span: Some(span),
            });
        }

        // R16: non-void function whose body does not return on
        // every path. First-pass: only structural ("last stmt is
        // Return") — path-completeness is queued.
        for (span, name) in self.missing_return_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::MissingReturnInNonVoidFunction { name },
                span: Some(span),
            });
        }

        // R17: return expression type must match the declared
        // return type. ESSL has no implicit conversions.
        for (span, expected, actual) in self.return_type_mismatch_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::ReturnTypeMismatch { expected, actual },
                span: Some(span),
            });
        }

        // R18: function redefinition with the same parameter types.
        for (span, name) in self.function_redefinition_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::FunctionRedefinition { name },
                span: Some(span),
            });
        }

        // R19: `attribute` declared in a fragment shader.
        for (span, name) in self.attribute_in_fragment_sites {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::AttributeInFragmentShader { name },
                span: Some(span),
            });
        }

        // R13: WebGL packing limits. Walk the global decls and sum
        // slot counts per storage class, comparing against the
        // stage's minimum-guaranteed limit. Conservative — uses one
        // slot per scalar/vec_n decl, n slots per mat_n decl, no
        // inter-declaration packing.
        let mut attr_slots: u32 = 0;
        let mut varying_slots: u32 = 0;
        let mut uniform_slots: u32 = 0;
        let mut first_attr: Option<Span> = None;
        let mut first_varying: Option<Span> = None;
        let mut first_uniform: Option<Span> = None;
        for d in &src.decls {
            let ExternalDecl::Global(g) = d else { continue };
            let slots = slot_count_for(g.ty.kind);
            if slots == 0 {
                continue;
            }
            match g.storage {
                crate::ast::StorageQualifier::Attribute => {
                    attr_slots += slots;
                    first_attr.get_or_insert(g.span);
                },
                crate::ast::StorageQualifier::Varying => {
                    varying_slots += slots;
                    first_varying.get_or_insert(g.span);
                },
                crate::ast::StorageQualifier::In if self.stage == ShaderStage::Vertex => {
                    attr_slots += slots;
                    first_attr.get_or_insert(g.span);
                },
                crate::ast::StorageQualifier::In => {
                    varying_slots += slots;
                    first_varying.get_or_insert(g.span);
                },
                crate::ast::StorageQualifier::Out if self.stage == ShaderStage::Vertex => {
                    varying_slots += slots;
                    first_varying.get_or_insert(g.span);
                },
                crate::ast::StorageQualifier::Uniform => {
                    uniform_slots += slots;
                    first_uniform.get_or_insert(g.span);
                },
                _ => {},
            }
        }
        if attr_slots > MAX_VERTEX_ATTRIBS && self.stage == ShaderStage::Vertex {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::PackingLimitExceeded {
                    class: "attribute",
                    used: attr_slots,
                    limit: MAX_VERTEX_ATTRIBS,
                },
                span: first_attr,
            });
        }
        if varying_slots > MAX_VARYING_VECTORS {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::PackingLimitExceeded {
                    class: "varying",
                    used: varying_slots,
                    limit: MAX_VARYING_VECTORS,
                },
                span: first_varying,
            });
        }
        let uniform_limit = match self.stage {
            ShaderStage::Vertex => MAX_VERTEX_UNIFORM_VECTORS,
            ShaderStage::Fragment => MAX_FRAGMENT_UNIFORM_VECTORS,
        };
        if uniform_slots > uniform_limit {
            result.errors.push(WebGlDiagnostic {
                severity: Severity::Error,
                kind: WebGlDiagnosticKind::PackingLimitExceeded {
                    class: match self.stage {
                        ShaderStage::Vertex => "vertex uniform",
                        ShaderStage::Fragment => "fragment uniform",
                    },
                    used: uniform_slots,
                    limit: uniform_limit,
                },
                span: first_uniform,
            });
        }

        // Render info_log. ANGLE / Chrome shape: one line per
        // error / warning, errors first, then warnings. Source text
        // resolves spans to 1-based line numbers.
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
                // R16: non-void function must end with a return on
                // every path. First-pass check is purely structural:
                // the last top-level body stmt must be a Return.
                // True path-completeness analysis is queued.
                if fd.return_ty.kind != TypeKind::Void
                    && !body_definitely_returns(&fd.body.stmts)
                {
                    self.missing_return_sites
                        .push((fd.span, fd.name.clone()));
                }
                // R17: track current return type so visit_stmt can
                // gate `return <expr>;` against it.
                self.current_return_ty = Some(fd.return_ty.kind);
                // R18: redefinition. If this name + param types
                // matches a previously-seen function, fire.
                if fd.name != "main" {
                    let param_kinds: Vec<TypeKind> =
                        fd.params.iter().map(|p| p.ty.kind).collect();
                    if let Some((existing_kinds, _)) =
                        self.function_signatures.get(&fd.name)
                    {
                        if existing_kinds == &param_kinds {
                            self.function_redefinition_sites
                                .push((fd.span, fd.name.clone()));
                        }
                    } else {
                        self.function_signatures
                            .insert(fd.name.clone(), (param_kinds, fd.span));
                    }
                }
            },
            Visit::Post => {
                self.current_function = None;
                self.current_return_ty = None;
            },
            Visit::In => {},
        }
        Walk::Continue
    }

    fn visit_global_decl(&mut self, g: &'tree GlobalDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            self.note_reserved_if_any(&g.name, g.name_span);
            self.note_precision_missing_if_any(
                &g.name,
                g.name_span,
                g.ty.kind,
                g.precision.is_some(),
            );
            // R19: `attribute` is vertex-only.
            if self.stage == ShaderStage::Fragment
                && g.storage == crate::ast::StorageQualifier::Attribute
            {
                self.attribute_in_fragment_sites
                    .push((g.span, g.name.clone()));
            }
        }
        Walk::Continue
    }

    fn visit_local_decl(&mut self, d: &'tree LocalDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre {
            self.note_reserved_if_any(&d.name, d.name_span);
            self.note_precision_missing_if_any(
                &d.name,
                d.name_span,
                d.ty.kind,
                d.precision.is_some(),
            );
            // R14: `const T x = <expr>;` requires <expr> to be a
            // constant expression. The first-pass acceptance set is
            // literal IntLit / FloatLit / BoolLit and unary / binary
            // expressions formed recursively from constants.
            if d.is_const {
                match &d.init {
                    None => {
                        self.const_without_init_named
                            .push((d.name_span, d.name.clone()));
                    },
                    Some(e) => {
                        if !is_constant_initializer(e) {
                            self.const_init_not_constant_named
                                .push((d.name_span, d.name.clone()));
                        }
                    },
                }
                self.const_locals.insert(d.name.clone());
            }
        }
        Walk::Continue
    }

    fn visit_precision_decl(&mut self, p: &'tree PrecisionDecl, visit: Visit) -> Walk {
        if visit == Visit::Pre && p.ty.kind == TypeKind::Float {
            self.float_default_set = true;
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
                // R12: indirect array index. ESSL 1.00 Appendix A
                // restricts array indices to "constant-index-
                // expressions". The first-pass acceptance set is
                // literal IntLit and currently-active loop
                // induction variable references. Gated to ESSL 1.00
                // (version == None or Some(100)).
                if self.is_essl_100() {
                    if let Expr::Index { index, .. } = e {
                        if !self.is_constant_index_expr(index) {
                            self.indirect_index_sites.push(index.span());
                        }
                    }
                }
                // R15: assignment to a `const` local. The LHS must
                // not name an identifier the validator has seen
                // declared `const` (tracked in `const_locals`).
                if let Expr::Assign { lhs, span, .. } = e {
                    if let Expr::Ident { name, .. } = lhs.as_ref() {
                        if self.const_locals.contains(name) {
                            self.const_assignment_sites
                                .push((*span, name.clone()));
                        }
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
        match visit {
            Visit::Pre => match s {
                Stmt::Discard { span } => {
                    if let Some(fname) = self.current_function {
                        self.discard_sites.push(DiscardSite { span: *span, function: fname });
                    }
                },
                Stmt::Return { value: Some(e), span } => {
                    // R17: return-expression type must match the
                    // enclosing function's declared return type.
                    // Skip when the typecheck did not annotate the
                    // expression (covered elsewhere) or the
                    // function's return type is Void (a stricter
                    // rule rejects `return <expr>;` in void, but is
                    // beyond R17's scope here).
                    if let (Some(expected), Some(actual)) = (
                        self.current_return_ty,
                        self.types.get(&e.span()).copied(),
                    ) {
                        if expected != TypeKind::Void && expected != actual {
                            self.return_type_mismatch_sites
                                .push((*span, expected, actual));
                        }
                    }
                },
                Stmt::For { span, init, .. } => {
                    if let Some(what) = check_for_appendix_a(s) {
                        self.for_loop_violations.push((*span, what));
                    }
                    // R12 prelude: push the loop induction variable
                    // name (when it can be determined). For loops
                    // that fail check_for_appendix_a still push so
                    // their body's `arr[i]` patterns are not also
                    // flagged as indirect.
                    if let ForInit::Decl(d) = init {
                        self.loop_var_stack.push(d.name.clone());
                    } else {
                        // Push a sentinel that no Ident will match;
                        // keeps push/pop balanced for Post.
                        self.loop_var_stack.push(String::new());
                    }
                },
                Stmt::Switch { discriminant, span, .. } => {
                    // R9: discriminant must be Int.
                    if let Some(ty) = self.types.get(&discriminant.span()).copied() {
                        if ty != TypeKind::Int {
                            self.switch_discriminant_bad.push((*span, ty));
                        }
                    }
                    // Open a fresh case-value set for this switch.
                    self.switch_case_stack.push(HashSet::new());
                },
                Stmt::Case { value, span } => {
                    // R10: case value must be a literal IntLit.
                    let int_value = match value {
                        Expr::IntLit { value, .. } => Some(*value),
                        _ => None,
                    };
                    match int_value {
                        Some(v) => {
                            // R11: duplicate within enclosing switch.
                            if let Some(set) = self.switch_case_stack.last_mut() {
                                if !set.insert(v) {
                                    self.case_duplicates.push((*span, v));
                                }
                            }
                        },
                        None => {
                            self.case_value_not_constant.push(*span);
                        },
                    }
                },
                _ => {},
            },
            Visit::Post => {
                if matches!(s, Stmt::Switch { .. }) {
                    self.switch_case_stack.pop();
                }
                if matches!(s, Stmt::For { .. }) {
                    self.loop_var_stack.pop();
                }
            },
            Visit::In => {},
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

// ---------- R16: non-void function must return on every path --------

/// First-pass structural check: the body's last top-level
/// statement is a `Stmt::Return`, OR the last stmt is a Block
/// whose last stmt is a Return (recursive), OR the last stmt is
/// an If with else where BOTH then- and else-stmts definitely
/// return.
///
/// Stops short of full path analysis (would handle e.g. `for { ...
/// return; }` plus an unreachable fallback, or switch-default
/// completeness). Conservative: false positives reject some legal
/// shaders; the receipts pin the boundary so a future widening
/// flips them.
fn body_definitely_returns(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(s) => stmt_path_returns(s),
        None => false,
    }
}

fn stmt_path_returns(s: &Stmt) -> bool {
    match s {
        Stmt::Return { value: Some(_), .. } => true,
        Stmt::Discard { .. } => true,
        Stmt::Block(b) => body_definitely_returns(&b.stmts),
        Stmt::If { then, else_: Some(else_), .. } => {
            stmt_path_returns(then) && stmt_path_returns(else_)
        },
        _ => false,
    }
}

// ---------- R14: constant-expression check for `const` inits ---------

/// First-pass acceptance set for an ESSL constant expression: any
/// literal, optionally combined recursively by unary or binary
/// operators. Identifiers (even `const`-bound ones) and calls are
/// not yet folded.
fn is_constant_initializer(e: &Expr) -> bool {
    match e {
        Expr::IntLit { .. } | Expr::FloatLit { .. } | Expr::BoolLit { .. } => true,
        Expr::Unary { expr, .. } => is_constant_initializer(expr),
        Expr::Binary { lhs, rhs, .. } => {
            is_constant_initializer(lhs) && is_constant_initializer(rhs)
        },
        _ => false,
    }
}

// ---------- R8: Float-family type check ------------------------------

fn is_float_family(ty: TypeKind) -> bool {
    matches!(
        ty,
        TypeKind::Float
            | TypeKind::Vec2
            | TypeKind::Vec3
            | TypeKind::Vec4
            | TypeKind::Mat2
            | TypeKind::Mat3
            | TypeKind::Mat4
    )
}

// ---------- R5: Reserved identifier prefix check ----------------------

fn reserved_reason(name: &str) -> Option<&'static str> {
    if name.starts_with("gl_") {
        return Some("starts with `gl_`");
    }
    if name.starts_with("webgl_") {
        return Some("starts with `webgl_`");
    }
    if name.starts_with("_webgl_") {
        return Some("starts with `_webgl_`");
    }
    if name.contains("__") {
        return Some("contains `__`");
    }
    if FUTURE_RESERVED.binary_search(&name).is_ok() {
        return Some("reserved for future use by ESSL §3.6");
    }
    None
}

/// ESSL 1.00 §3.6 future-reserved keyword set. Names a shader is
/// forbidden from using as identifiers because a future ESSL
/// version may take them as keywords. Sorted for binary search.
/// Notes: `switch`, `default`, `case`, `inline`, `noinline` are
/// reserved in 1.00 but used in 3.00 — the parser handles those
/// version-gated cases; the validator only flags names that are
/// reserved in BOTH versions.
const FUTURE_RESERVED: &[&str] = &[
    "active", "asm", "cast", "class", "common", "dvec2", "dvec3", "dvec4",
    "enum", "extern", "external", "fixed", "fvec2", "fvec3", "fvec4", "goto",
    "half", "hvec2", "hvec3", "hvec4", "input", "interface", "long",
    "namespace", "output", "packed", "partition", "public", "sampler1D",
    "sampler1DShadow", "sampler2DRect", "sampler2DRectShadow", "sampler2DShadow",
    "sampler3D", "sampler3DRect", "short", "sizeof", "static", "superp",
    "template", "this", "typedef", "union", "unsigned", "using", "volatile",
];

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
