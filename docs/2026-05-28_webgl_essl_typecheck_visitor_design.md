# webgl-essl Step 4: typecheck visitor design

A design sketch for the typecheck pass that sits above the parse tree.
Borrows the visitor shape from mozangle's `TIntermTraverser` after
reading the header in the cargo registry (see "What I read").

Companion to:

- [`2026-05-28_webgl_essl_rust_frontend_spike.md`](2026-05-28_webgl_essl_rust_frontend_spike.md): the parent plan; this doc fleshes out Step 4.

---

## 1. What I read

`mozangle-0.5.5/gfx/angle/checkout/src/compiler/translator/tree_util/IntermTraverse.h`
(+ `Visit.h`, + `IntermTraverse.cpp` for the actual firing order).

`TIntermTraverser` is a base class whose subclasses override
`visit*(Visit, TIntermNode*) -> bool` methods, one per node kind
(Binary, Unary, Ternary, IfElse, Switch, FunctionDefinition, Aggregate,
Block, Declaration, Loop, Branch, plus leaf overloads for Symbol /
ConstantUnion / FunctionPrototype / PreprocessorDirective). The
default returns true.

Key shape choices worth keeping:

- **Three-phase visit.** `enum Visit { PreVisit, InVisit, PostVisit }`.
  PreVisit fires before descending; InVisit fires between siblings of
  the current parent; PostVisit fires after the subtree is done. Lets
  one visit function handle scope enter / scope exit / between-children
  bookkeeping without splitting into three traits.
- **Return false from PreVisit to skip the subtree.** Lets a visitor
  short-circuit branches it does not care about, like the dead branches
  of a constant-folded ternary.
- **Path tracking with RAII.** `ScopedNodeInTraversalPath` pushes the
  current node onto `mPath` on construction, pops on drop. `mMaxDepth`
  paired with `mMaxAllowedDepth` bounds stack growth against
  adversarial deeply nested input. `getParentNode()` /
  `getAncestorNode(n)` read off the path for context-sensitive checks.
- **Per-child index tracking.** `mCurrentChildIndex` tells the visitor
  which child of the current parent is being entered or exited; useful
  in InVisit ("we just finished the `then` branch, next is `else`").
- **Symbol table is a constructor argument**, owned by the caller and
  borrowed by the traverser. Multiple passes share one symbol table.
- **Tree mutation via deferred replacements.** Visitors push to
  `mReplacements` / `mMultiReplacements` during traversal; the caller
  applies them via `updateTree()` after. This is what lets the typecheck
  pass evolve into the constant-folding and desugaring passes Step 5
  needs without rewriting the walker.

## 2. Lessons borrowed for Rust

What carries over:

- The three-phase enum and the bool-skip-subtree contract.
- Visitor trait with one method per node kind, default = continue.
- Path tracking on the visitor (Rust borrow-checker friendly: `Vec<&'tree Node>` keyed off the parse-tree lifetime).
- Symbol table on the visitor, frame-stack shape.
- Deferred mutations queued during traversal, applied after.

What does not:

- Object-pool allocation. The Rust `Box<Expr>` / `Vec<Stmt>` pattern is
  already the natural ownership; no pool allocator.
- C++ virtual dispatch via `visit*` overrides. Trait dispatch is the
  Rust equivalent. Performance: the small constant factor difference
  is below noise for a frontend pass.
- ANGLE's `TIntermNode` runtime type tag. Rust `enum` is the same
  information at compile time. The match arms in our walk functions
  replace the virtual `traverse()` overloads.

## 3. Proposed Rust visitor shape

A new `webgl_essl::visit` module exposes a single `Visitor` trait, a
`Visit` enum, a `Walk` control-flow enum, and one `walk_*` function per
top-level node type.

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Visit { Pre, In, Post }

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Walk { Continue, Skip }

pub trait Visitor<'tree> {
    fn visit_translation_unit(&mut self, _node: &'tree TranslationUnit, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_external_decl(&mut self, _node: &'tree ExternalDecl, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_function_def(&mut self, _node: &'tree FunctionDef, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_struct_decl(&mut self, _node: &'tree StructDecl, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_global_decl(&mut self, _node: &'tree GlobalDecl, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_block(&mut self, _node: &'tree Block, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_stmt(&mut self, _node: &'tree Stmt, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_local_decl(&mut self, _node: &'tree LocalDecl, _visit: Visit) -> Walk { Walk::Continue }
    fn visit_expr(&mut self, _node: &'tree Expr, _visit: Visit) -> Walk { Walk::Continue }
}

pub fn walk_translation_unit<'tree, V: Visitor<'tree>>(v: &mut V, tu: &'tree TranslationUnit) {
    if matches!(v.visit_translation_unit(tu, Visit::Pre), Walk::Skip) { return; }
    for decl in &tu.decls { walk_external_decl(v, decl); }
    v.visit_translation_unit(tu, Visit::Post);
}

// ... walk_external_decl, walk_function_def, etc.
```

Notes:

- Lifetime `'tree` carries the parse tree's lifetime through to the
  visitor methods, so visitors can hold `&'tree Node` references in
  their state.
- `Walk::Skip` from PreVisit skips children; from In or Post it is a
  no-op (matching ANGLE's bool return contract).
- Path tracking is the visitor's responsibility, not the walker's. The
  walker stays stateless and inlinable.
- Tree mutation: the walker takes `&'tree TranslationUnit` (read-only).
  Mutation-style passes build a parallel "annotations" map keyed on
  `Span` (which is unique per node). When a real mutation pass arrives
  (Step 5 constant folding), we add a `VisitorMut` trait that takes
  `&'tree mut TranslationUnit` and define `walk_*_mut` siblings.

## 4. First typecheck pass scope (minimum-viable)

What the first `TypeChecker` visitor must do, in order of value-per-cost:

1. **Symbol resolution.** A `Scope` stack on the visitor; `scope.define(name, kind, span)` from local decls and function params; `scope.lookup(name)` returns the declaration node. Issue `UnknownIdentifier` diagnostic on miss.
2. **Literal types.** `IntLit` → `int`, `FloatLit` → `float`, `BoolLit` → `bool`.
3. **Ident expression types.** Resolved through the scope stack.
4. **Binary op result types.** ESSL is strict: no implicit conversions between `int` and `float`. Build a small table: `(BinOp, TypeKind, TypeKind) -> Option<TypeKind>`. Issue `OperandTypeMismatch` on miss.
5. **Assignment.** LHS must be an l-value (ident, member access, index); RHS type must equal LHS type. Issue `NotAnLValue` and `AssignTypeMismatch`.
6. **Constructor calls.** `vec4(args)` accepts: 4 scalars, or 2 vec2s, or 1 vec3 + 1 scalar, or 1 vec4 (copy), etc. Build the table from ESSL spec §5.4.2; result type is the constructed type.
7. **Function calls.** Resolve callee name in the function-scope (or built-in registry); check arity and per-arg type match. Built-ins (`texture2D`, `sin`, `mix`, `dot`, etc.) ship as a hardcoded registry mapping name to signature, sourced from ESSL 1.00 §8.

Out of the first pass:

- Implicit type coercions (ESSL has none in 1.00, so this is a non-issue).
- Function overload resolution. ESSL 1.00 has limited overloading; punt to Step 4b if a real shader surfaces it.
- Struct field type lookup. Parser already stores struct decls; first pass treats `<expr>.<field>` as `Unknown` if `<expr>` is a struct type. Step 4b handles it.
- Array sized declarators. ESSL 1.00 has them; Step 4b.

## 5. What this gets us, what it doesn't

What it gets us:

- A real diagnostic stream from `webgl_essl::check(tu)`, shaped like
  `Vec<TypeDiagnostic>` with span + message. This is the first concrete
  building block of the "match mozangle behavior" differential gate
  (Step 4 done condition).
- A walker infrastructure that Step 5 (WebGL validator restrictions,
  hostile-input mitigations) reuses without rewriting.
- An annotated-types map (`HashMap<Span, TypeKind>`) ready for the
  lowering pass to consume in Step 6.

What it does not get us:

- WebGL-spec-shaped errors. The first pass emits `webgl_essl`-native
  diagnostics; the `getError()` shape comes in Step 5 with the WebGL
  validator layer on top.
- Constant folding / dead-code elimination. Step 5.
- Anything ESSL 3.00. Step 3.

## 6. Open questions

- **Built-in registry shape.** A hand-written table of `(name, sig)` for
  the ~150 ESSL 1.00 built-ins is real work but localized. Alternative:
  parse the ESSL spec's built-in section into a registry at build time
  from a vendored text file. Decide when implementing.
- **Diagnostic surface.** Reuse `webgl_essl::error::Error` (extending
  `ErrorKind`) or introduce a separate `TypeDiagnostic`? Lean toward the
  latter so the parser's error vocabulary stays focused on syntax.
- **Span as annotation key.** `HashMap<Span, TypeKind>` is the obvious
  shape, but ASTs with multiple nodes at the same byte range would
  collide. Investigate when annotating; may need a node id instead.
- **Should the visitor be `Visitor<'tree>` or take `&Node` parameters
  without a tree lifetime?** Lifetime threading is correct but ergonomic
  cost may bite. Decide by writing the first pass and seeing.

## 7. Next concrete step

Add `webgl_essl::visit` module: `Visit` and `Walk` enums, the `Visitor`
trait with default impls, and the `walk_*` functions for the seven
top-level node types. No `TypeChecker` yet; just the infrastructure +
a smoke test that walks a parsed translation unit and counts nodes per
kind. Then Step 4 proper writes `TypeChecker` against that
infrastructure.
