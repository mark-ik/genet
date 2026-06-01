# webgl-essl: a pure-Rust ESSL frontend spike

The first rung of a possible pure-Rust replacement for ANGLE's shader
translator. Lives at `components/webgl-essl/`, package `servo-webgl-essl`,
lib `webgl_essl`. Sits beside `webgl-wgpu` (the GPU adapter) rather than
inside it: the adapter consumes the parse tree this crate produces.

Companion to the WebGL plan in netrender
([2026-05-06_webgl_over_wgpu_plan.md](../../netrender/netrender-notes/2026-05-06_webgl_over_wgpu_plan.md)).
This doc owns the Rust-frontend lane; mozangle stays available as the
differential oracle.

---

## 1. Why pure-Rust, not mozangle in production

The runtime constraint is pure Rust + wgpu. mozangle preserves runtime
purity (shader-only build, no EGL / GLES runtime), but adds a C++ build
dependency to every target that builds WebGL, including the wasm32 lane
the Lane C fullweb tier eventually opens. A Rust frontend removes that
build dependency, lets us own every spec corner, and donates an
ecosystem-missing piece (no Rust crate is a browser-grade ESSL
validator today).

mozangle still has a role: behavioral oracle. Every shader fed to this
crate is also fed to mozangle, outputs and errors compared. The Rust
crate ships pieces as each clears WebGL CTS, and mozangle becomes
oracle-only, then leaves the build graph when the Rust crate is the
production path for long enough to trust.

## 2. The mountain — steps and targets

Each step has a done condition that resolves at code-review time, not a
calendar bet.

- **Step 1 — Parser receipt (canonical corpus).** `lex` + `parse`
  accepts the canonical-triangle vertex / fragment pair plus the
  extended-corpus shapes (uniform / varying / binary `*`) and produces
  an AST whose structure tests assert directly. *Done condition:*
  receipts at `tests/canonical_triangle.rs` pass. *Status: shipped, 6/6
  green.*

- **Step 2 — Parser breadth.** Extend the grammar to cover the WebGL 1
  shader corpus an embedder is likely to encounter: `if` / `for` /
  `while`, struct decls, swizzles (`.xyz`), texture sampling
  (`texture2D` / `textureCube`), built-in math, unary prefixes, ternary.
  *Done condition:* a curated corpus (WebGL Conformance Suite shader
  examples, plus the corner cases mozangle's tests exercise) parses
  cleanly, with no `Unsupported` errors. *Status: primary grammar
  coverage complete; conformance corpus is the next gate.*
  - **First pass shipped.** Pratt refactor of the expression layer
    (binding-power table + one driver, lessons borrowed from chumsky's
    `pratt` module and matklad's "Simple but Powerful Pratt Parsing");
    unary prefix (`-` / `+` / `!` / `++` / `--`) and postfix
    (`++` / `--`); member access (`.field` covers struct fields and
    swizzles), index (`[expr]`), call-from-arbitrary-ident; `if` /
    `else`, `while`, `do` / `while`, `for` with `ForInit::{Empty, Decl,
    Expr}`; `break` / `continue` / `discard`; local declarations with
    `const` and precision qualifiers; compound block as statement.
    Receipts at `tests/step2_breadth.rs`, 17/17 green. parse.rs split
    into `parse/{decl,stmt,expr}.rs` once it crossed 600 LOC; each
    submodule under 330 lines.
  - **Second pass shipped.** Function definitions with non-empty
    parameter lists (the path existed; first explicit receipt confirms
    it); ternary `?:` (right-associative, precedence between assign and
    log-or, mixfix-handled inline in the Pratt loop with `TERNARY_BP`);
    struct declarations at file scope (`struct Name { field; field; };`
    with multi-name-per-line via `vec3 a, b;`). Receipts at
    `tests/step2_remainder.rs`, 8/8 green.
  - **Still open at this gate.** Conformance corpus actually run
    against the parser (the Step 2 done condition); `texture2D` /
    `textureCube` (lex as identifiers already; validator gives them
    built-in semantics); the long tail of WebGL 1 shader idioms the
    corpus will surface.

- **Step 3 — ESSL 3.00 parser delta.** ESSL 3.00 adds `in` / `out` /
  `centroid` / `flat` / `smooth`, layout qualifiers, integer literal
  suffixes, bitwise ops, shift ops, `switch`. *Done condition:* the
  WebGL 2 conformance shader subset parses.

- **Step 4 — Symbol table + type checker.** ESSL has implicit
  conversions, sampler types, array sizing, structure fields, function
  overload resolution, and built-in symbol injection. *Done condition:*
  for every shader in the conformance corpus, this crate's `getError()`-
  shaped diagnostic stream matches mozangle's (modulo wording).

- **Step 5 — WebGL validation layer.** The restrictions that make valid
  ESSL invalid WebGL ESSL: no recursion, restricted `for` loop forms,
  precision-of-builtins rules, packing restrictions, expression-
  complexity limit, call-stack-depth limit, indirect-array clamping.
  Each maps to an AST / IR pass with a feature flag matching mozangle's
  `CompileOptions`. *Done condition:* hostile-input shaders from the
  conformance suite produce the same accept / reject verdict as mozangle.

- **Step 6 — Lowering.** AST → IR → wgpu pipeline. Three viable targets,
  pick at this step:
  - **A. SPIR-V via [`rspirv`](https://crates.io/crates/rspirv), then naga `spv-in`.** Smallest cut. naga's
    SPIR-V frontend does the IR work; we emit SPIR-V from the AST.
  - **B. naga `Module` builder direct.** Skip SPIR-V; build naga IR
    from the AST. Saves a hop, but naga's IR builder isn't designed as
    a third-party-frontend target.
  - **C. Own IR + own backends.** Most flexibility, most code.
  *Done condition:* canonical-triangle vertex / fragment round-trip
  through the chosen lowering produce a wgpu pipeline that renders the
  expected triangle.

- **Step 7 — WebGL CTS.** Wire `conformance/glsl/*` and friends as a
  test suite. Ratchet by category (compilation, linking, errors,
  reflection). *Done condition:* the categories the suite covers all
  pass; the remaining gaps are documented as either spec-incomplete
  (named in this doc) or upstream-blocked (named with reason).

- **Step 8 — Production path swap.** `webgl-wgpu` switches from its
  current canonical-pair recognizer to `webgl_essl::parse_source` + the
  step-6 lowering. mozangle stays in the build graph as the differential
  oracle, gated behind a cargo feature, run in CI on every WebGL change.
  *Done condition:* the W4 paint receipt
  (`webgl_canvas_texture_e2e.rs`) still passes with the Rust-frontend
  path driving the canvas texture.

- **Step 9 — mozangle removed.** When step 8 has been the production
  path long enough for the differential CI job to be silent for K
  consecutive WebGL CTS runs, mozangle leaves the build graph. *Done
  condition:* the cargo feature stops being referenced; the build
  graph has no C++ on the WebGL path.

## 3. ANGLE-as-oracle differential framing

mozangle ships today and is spec-correct enough that real browsers use
it. The Rust crate's correctness target is "matches mozangle's
behavior." Every step after 1 has a differential probe: run input
through both, diff outputs. Disagreements are bugs (in either crate; we
investigate before assuming the Rust side is wrong).

Differential probe lives in a separate crate or feature, never inside
`webgl-essl` itself, so the Rust frontend's tests stay buildable on
vanilla Windows / wasm32 / anywhere mozangle's C++ build is out of
reach.

## 4. What this spike crate ships today

- Byte-level lexer: identifiers, ESSL keywords, integer + float
  literals (incl. exponents and trailing `f`), line + block comments,
  single + double character punctuation, the operator set the WebGL 1
  precedence table needs.
- AST: translation unit, external decls (precision, global, function,
  struct), parameters, types, storage and precision qualifiers, blocks,
  statements (expr, return, decl, block, `if`, `while`, `for`, `do`,
  `break`, `continue`, `discard`), expressions (literals, idents,
  calls, assignments incl. compound, binary ops, unary prefix + postfix,
  member access, indexing, ternary; precedence-respecting via a Pratt
  driver + binding-power table).
- Parser: recursive-descent for declarations and statements (in
  `parse/decl.rs` and `parse/stmt.rs`); single Pratt loop per
  precedence level for expressions (in `parse/expr.rs`). Errors carry
  spans; an `Error::display(src)` adapter renders 1-based line / column.
- Tests: canonical-triangle vertex + fragment, tinted vertex + fragment
  with uniform / varying / binary `*` arg, comment skipping, malformed-
  input error path (6 tests); Step 2 breadth corpus exercising local
  decls (with / without init, `const`), `if` / `else`, nested if-else
  chain, `while`, `do` / `while`, `for` with decl init / empty slots,
  jump statements inside loops, swizzle access, swizzle after call,
  unary `-` on float lit, unary `!` on bool lit, postfix `++` in
  for-step, additive vs multiplicative precedence (17 tests); Step 2
  remainder exercising function definitions with three params and with
  `(void)` form, two-function translation units, ternary in assign-
  rhs, right-associativity, log-or binding tighter than ternary, struct
  decls with three typed fields and with multi-name-per-line (8 tests).
  31/31 green on Windows.

## 5. What it doesn't ship (and the order to add)

For Step 2 closure: run the WebGL Conformance Suite shader corpus
through `parse_source` and fix whatever surfaces. For Step 3: the
ESSL 3.00 delta (shift / bitwise / `in` / `out` / `centroid` / layout
qualifiers / integer literal suffixes / `switch` / arrays with sized
declarators / `length()` postfix). Each is a localized addition: one
row in the binding-power table, one statement arm, or one decl variant.

Also still parser-side but not exercised yet: struct types as type
specifiers in declarations (the validator's symbol table resolves a
struct name to a type; the parser today only recognizes built-in type
keywords).

## 6. Lowering target — when to decide

Step 6 picks between paths A / B / C. The choice is reversible at code
level (the AST is the contract above; the lowering is internal), but
the work fans out from it, so park the decision until the type checker
and validator are in (step 5). At that point the AST shape is stable
and we know how much IR-construction ergonomics we need from the chosen
target.

If we delay the choice and want to ship a step-6 receipt early, the
default is path A (SPIR-V via `rspirv`). It's the smallest cut and
leans on naga's existing SPIR-V frontend. Naga as the IR engine, our
crate as the WebGL-shaped frontend over it.

## 7. Lessons borrowed

Per [borrow-technique-from-mature-libs feedback], the parser is
hand-rolled but reads other crates' shapes before extending. So far:

- **chumsky's `pratt` module.** `Parser::pratt` accepts an atom plus a
  tuple of `prefix` / `infix` / `postfix` operator entries, each
  carrying a numeric binding power and associativity (`left(n)`,
  `right(n)`, `none(n)`). The shape generalizes to ESSL's 14
  precedence levels as a single lookup table rather than 14 functions.
  Lifted: the binding-power table as the source of truth, the
  driver-loop shape. Not lifted: the type machinery (chumsky's
  combinator types are far more general than we need).
- **matklad's "Simple but Powerful Pratt Parsing".** The canonical
  reference for the `(l_bp, r_bp)` pair convention and the climbing
  loop. Cited inline in `parse/expr.rs`.

Planned reads as the project moves up the mountain:

- **mozangle's `TIntermTraverser`** before writing the typecheck visitor
  (step 4).
- **ANGLE's `ParseContext`** restrictions table before the WebGL
  validator layer (step 5).
- **`rspirv` builder examples** plus **`naga::Module` construction
  sites** before picking lowering path A vs B (step 6).

## 8. Related

- The WebGL plan §3: [2026-05-06_webgl_over_wgpu_plan.md](../../netrender/netrender-notes/2026-05-06_webgl_over_wgpu_plan.md).
  This crate replaces §3's "naga `glsl-in`" path; the extend-and-shed
  port plan in §3.4 remains the fallback if step 1's parser breadth
  proves intractable, which the receipt suggests it will not.
- Two-lanes framing: [2026-05-29_serval_two_lanes.md](2026-05-29_serval_two_lanes.md).
  WebGL is Lane C content; Mere's orrery is not a WebGL surface.
- WebGL adapter today: [components/webgl-wgpu/lib.rs](../components/webgl-wgpu/lib.rs)
  and the W4 paint receipt at
  [components/paint/tests/webgl_canvas_texture_e2e.rs](../components/paint/tests/webgl_canvas_texture_e2e.rs).
- The borrow doctrine: feedback memory `borrow-technique-from-mature-libs`.
