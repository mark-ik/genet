# layout_dom_api — crate location & trait shape (design, for review)

**Status (2026-05-16):** proposed; for review. Resolves the first two open decisions in [2026-05-16_serval_layout_lift_plan.md](./2026-05-16_serval_layout_lift_plan.md) (path C, P2.2): where `LayoutDom` lives, and what shape its trait takes.

Supersedes [2026-05-13_p2_layout_dom_provider_design.md](./2026-05-13_p2_layout_dom_provider_design.md), which described the equivalent seam inside the now-dead `components/layout/` crate.

---

## Decision 1 — separate `layout_dom_api` crate

`LayoutDom` lives in a new `components/shared/layout-dom/` crate, package name `layout-dom-api`. Not in `layout_api`.

### Plausible consumers beyond `serval-layout`

The decision rule from the lift plan is "separate crate iff there's a plausible additional consumer." At least three are real, not speculative:

1. **Reader-mode / smolweb-extract head** of three-head Hekate. The extract head walks the DOM for readability scoring, never invokes layout. Pulling in `layout_api` (with its Painter / PaintWorklet / `LayoutHostServices` surface) for a crate that only wants tree-walking is overhead.
2. **DOM serialization** (outerHTML, view-source, save-as-HTML). One implementation over any `LayoutDom` impl is cleaner than per-backend serializers. Doesn't need layout types.
3. **Selector matching** (`querySelector` analog). The `selectors` crate is already a workspace dep; building a `LayoutDom::query_selector` helper crate above `layout_dom_api` is more useful than coupling it to layout.

Less concrete but plausible: devtools/Apparatus-style DOM inspector for mere, static-analysis lints, accessibility-tree construction that wants to be layout-free.

### What stays in `layout_api`

Painter / DrawAPaintImageResult / PaintWorkletError, `LayoutHostServices` / `NoOpLayoutHostServices`, `LayoutConfig`, `LayoutFactory`, the `Layout` trait itself. These are the layout *engine's* interface to its host; they don't belong above a DOM-only consumer.

### Cost

One additional crate to maintain. Cargo.toml + lib.rs scaffold cost is negligible; the real cost is one extra workspace.dependencies entry and one extra place to look. Worth it.

---

## Decision 2 — hybrid pattern: opaque IDs + visitor with default walk

The DOM-traversal trait uses **pattern C as foundation** (opaque `NodeId` + lookup methods) **with pattern B layered on top** (visitor trait with a default walk implementation over the C primitives). **No pattern A** (no per-node-kind handle types like `Self::Element`, `Self::Text`).

### The three patterns recapped

- **A (handles):** `type Element: ElementRef; fn document_element(&self) -> Option<Self::Element>`. Servo, Blitz, html5ever's parser-side types, ego_tree.
- **B (visitor / cursor):** `fn walk(&self, v: &mut dyn NodeVisitor)`. Backend owns iteration. Visitor methods do the per-node work.
- **C (IDs + lookups):** `type NodeId: Copy; fn children(&self, id: Self::NodeId) -> impl Iterator`. petgraph, slotmap, most graph libraries.

### Trade-off summary

| Concern | A | B | C |
| --- | --- | --- | --- |
| Familiar to Rust devs | yes | no | yes |
| Backing-store independence | no (handle types leak) | yes | yes |
| Async / wasm friendliness | weak (lifetimes across awaits) | yes | yes |
| Per-kind type safety | yes | partial | no (matches NodeKind enum) |
| Backend controls iteration | no | yes | no |
| Random access by identity | yes | needs parallel API | yes |
| Iterator/functional composition | yes | no | yes |
| Backtracking / look-ahead | yes | cursor yes; visitor no | yes |
| Lift cost from servo-derived code | low | high | medium |

The hybrid (C + B-default) keeps the wins of C (familiar, identity-clean, backing-store opaque) while letting B layer on for walks that want backend-driven traversal.

### Real-world examples

To get a feel for this pattern, the closest existing reads are:

- **`petgraph::visit`** ([docs.rs/petgraph/latest/petgraph/visit/](https://docs.rs/petgraph/latest/petgraph/visit/)). `GraphBase` has `NodeId` / `EdgeId` associated types. `IntoNeighbors`, `IntoNodeIdentifiers` give iteration. `Dfs`, `Bfs`, `DfsPostOrder` are reified cursor types. `Visitable` plus `depth_first_search()` lets you supply per-event callbacks. **Closest "feel" match to what we're proposing.** Read this one first.
- **`rustc_hir::intravisit::Visitor`** (in [rust-lang/rust](https://github.com/rust-lang/rust)). `HirId` is opaque identity; `Visitor` has `visit_expr`, `visit_stmt`, etc., each defaulting to `walk_expr`, `walk_stmt`. Override visits to do work, fall through walks for plain descent. `HirMap` gives ID-based lookups. **Production-grade, large codebase, exactly this pattern.** Search `intravisit.rs` and `walk_*` to see the default-walks idiom.
- **`tree-sitter`** ([tree-sitter.github.io](https://tree-sitter.github.io/tree-sitter/)). C library, but the API design is identical philosophy: opaque `Node` handles (effectively IDs) + `TreeCursor` for streamed walking + S-expression queries for pattern matching. No per-node-kind handle types in the public API.
- **`html5ever::tokenizer::TreeSink`** ([docs.rs/html5ever](https://docs.rs/html5ever)). Already in our dep graph; `serval-static-dom`'s `StaticTreeSink` implements it. Inverted form (parser drives sink), but sink uses opaque `Handle` type, no per-kind handles. The pattern is already familiar to anyone reading serval-static-dom.
- **`swc_visit`** ([github.com/swc-project/swc](https://github.com/swc-project/swc)). Less pure (typed node structs, not opaque IDs) but the default-walks-via-derive-macro idiom is the same. Worth a glance for "how does the visitor surface scale as the node taxonomy grows."

Of these, **petgraph is the easiest read** and **rustc HIR is the closest production analog**. If we're picking one to mimic, mimic the HIR pattern.

### Concrete sketch

```rust
// In components/shared/layout-dom/lib.rs

pub trait LayoutDom {
    type NodeId: Copy + Eq + Hash + Debug + 'static;

    fn document(&self) -> Self::NodeId;
    fn kind(&self, id: Self::NodeId) -> NodeKind<'_>;
    fn parent(&self, id: Self::NodeId) -> Option<Self::NodeId>;
    fn children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_;

    // Default walk over the C primitives. Backends override if they want
    // backend-driven traversal (parallel layout pass, prefetching, etc.).
    fn walk<V: NodeVisitor<Self>>(&self, visitor: &mut V) {
        walk_subtree(self, self.document(), visitor)
    }
}

pub enum NodeKind<'a> {
    Document,
    Doctype { name: &'a str, public_id: &'a str, system_id: &'a str },
    Element(ElementView<'a>),
    Text(&'a str),
    Comment(&'a str),
    ProcessingInstruction { target: &'a str, data: &'a str },
}

pub struct ElementView<'a> {
    pub name: &'a QualName,
    pub attrs: &'a [Attribute],
}

pub trait NodeVisitor<D: LayoutDom + ?Sized> {
    fn enter(&mut self, dom: &D, id: D::NodeId) -> Walk { Walk::Descend }
    fn exit(&mut self, dom: &D, id: D::NodeId) {}
}

pub enum Walk { Descend, Skip, Stop }

fn walk_subtree<D, V>(dom: &D, root: D::NodeId, v: &mut V)
where D: LayoutDom + ?Sized, V: NodeVisitor<D>
{
    match v.enter(dom, root) {
        Walk::Skip | Walk::Stop => return,
        Walk::Descend => {
            for child in dom.children(root) {
                walk_subtree(dom, child, v);
            }
            v.exit(dom, root);
        }
    }
}
```

Notes on the sketch:

- `NodeKind<'a>` borrows from the backing store; no allocation per node access. `ElementView<'a>` carries the hot fields; the cold ones (template contents, integration-point flags) get separate accessor methods if needed.
- `walk` has a default impl; the simplest backend (StaticDocument's Rc-tree) gets it for free.
- Visitor methods default to "descend" + "no-op exit" — empty visitors that just want to count nodes are one-liners.
- The recursion bound is the DOM depth; pathological inputs (deeply nested HTML) bottom out in the default walker. If that becomes a real problem, the default impl can switch to an explicit stack. Out of scope for first cut.

### Caveats and cost

1. **Less familiar to Servo-derived contributors.** Existing `components/layout/` code reaches for `LayoutNode::parent_node()` directly; the new shape reaches for `dom.parent(id)`. Mental shift is small but real.
2. **Lift cost from pattern-A code is higher than a pattern-A trait would impose.** Estimate: P2.3 takes 10–20% more time than a straight port. Mitigation: the port is batch-by-batch anyway; each batch absorbs the shape change in isolation.
3. **Pattern-A's "this handle is definitely an Element" type safety is lost.** Mitigation: `kind()` returns an enum; `Walk::Skip` lets visitors bail early on non-matching nodes. The matches in traversal code are unavoidable anyway (you're checking node type before doing per-kind work in either pattern).
4. **Random access patterns work fine** because IDs are first-class. `querySelector` returns an `Option<NodeId>`; hit testing returns a `NodeId`; caller does `dom.kind(id)` to dispatch.
5. **`Send + Sync` decisions are pushed down to the impl.** `LayoutDom` doesn't require either; per-backend choice. `StaticDocument` is `!Sync` (Rc-tree). A future scripted DOM will need to be `Sync` if `LayoutHostServices` keeps its Sync bound — but that's a problem for whenever scripted lands, not now. See the P1 fallout in the strategy doc for the historical version of this Sync conversation.

### Exit criteria — when we'd abandon this and switch to pattern A

If any of these turn out true during P2.3, the pattern isn't paying its costs and we revert to pattern A:

- The match-on-NodeKind sites become a measurable hot path (after profiling, not before).
- The lift cost balloons past 30% extra vs. a pattern-A baseline because too much existing layout code wants direct typed-handle access.
- Stylo's integration points genuinely can't bridge to opaque IDs without a typed-handle adapter shim that's bigger than the trait itself.
- A scripted-DOM provider arrives and its Sync requirements push us into a corner the pattern can't accommodate.

Reversal is straightforward — `layout_dom_api` is a young crate, callers are few, the trait surface is small. The cost is rewriting `serval-static-dom`'s impl and any `serval-layout` callsites that consumed the trait. Bisect-friendly history means we can identify and back out cleanly.

---

## Wider applicability (informational, not part of this decision)

If this pattern works for `LayoutDom`, the same hybrid (opaque IDs + visitor with default walk) is a candidate for other identity-vs-walk APIs in the ecosystem:

- *serval-layout's fragment tree* — IDs for hit-testing, visitor for paint emission.
- *netrender's display list* — IDs for layer/clip references, visitor for the render pass.
- *mere's panel registry* — already ID-based; gain a visitor for "walk all panels" if useful.
- *mere/graphshell graph crate* — IDs + visitor matches petgraph and the prior graphshell work.
- *eidetic's content store* — IDs already; visitor would make `walk all stored content` streamable.

The hidden win: a **consistent decision rule** for new APIs across the ecosystem — "Identity operation? Add a method on the owning struct keyed by ID. Walk? Add a visitor trait with a default impl over the IDs." Reduces design churn at every new crate.

This is not a commitment for those crates yet. Validate the pattern in `layout_dom_api` first; if it carries its weight, propose extending it elsewhere.

---

## Open questions for review

1. **NodeKind shape.** Should `ElementView` include attrs inline (`attrs: &[Attribute]`) or expose an `attrs(id) -> impl Iterator`? Inline is simpler; iterator scales better if a backend stores attrs in a separate column.
2. **Attribute lookup.** `LayoutDom::attribute(id, name)` as a primitive, or build it from `ElementView::attrs`? Stylo's selector matching reads attributes hot — primitive form may matter for perf.
3. **Mutation surface.** This sketch is read-only. Scripted DOM needs mutation (innerHTML, appendChild, etc.). Decide later whether mutation goes in a `LayoutDomMut: LayoutDom` extension trait or in a separate trait. Static profile doesn't care.
4. **Crate name.** `layout-dom-api` vs. `layout-dom` vs. `serval-layout-dom`. Following the existing pattern (`layout_api`, `paint_api`, `script_traits`) — pick `layout-dom-api`. Package name uses hyphens; Rust import `use layout_dom_api::...` uses underscores.
5. **Where the default `walk` impl lives.** Free function (`pub fn walk_subtree<D: LayoutDom>(...)`) or default method on the trait? Default method is more discoverable; free function is more flexible. Lean default method.

---

## Review checklist

- [ ] Are the three plausible consumers (reader-mode, serialization, querySelector) real enough to justify a separate `layout_dom_api` crate, or should we wait for one to actually exist?
- [ ] Are petgraph and rustc HIR sufficient prior art, or do we want to mock up a minimal `serval-static-dom` impl against this trait first to see how it feels?
- [ ] Is the 10–20% lift-cost premium worth the wins (backing-store independence, async/wasm, no handle proliferation)?
- [ ] Is the `Send + Sync` question genuinely deferrable to "whenever scripted lands," or does it shape the trait now?
- [ ] Are the exit criteria (when we'd revert to pattern A) tight enough that we'd actually notice and act?
