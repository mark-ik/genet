# layout_dom_api — crate location & trait shape (design)

**Status (2026-05-16, revised):** decision adopted following a review pass. Resolves the first two open decisions in [2026-05-16_serval_layout_lift_plan.md](./2026-05-16_serval_layout_lift_plan.md) (path C, P2.2): where `LayoutDom` lives, and what shape its trait takes.

Supersedes [archive/2026-05-13_p2_layout_dom_provider_design.md](./archive/2026-05-13_p2_layout_dom_provider_design.md), which described the equivalent seam inside the now-dead `components/layout/` crate (archived 2026-05-17).

**Revision history:**

- 2026-05-16 (initial): proposed; for review.
- 2026-05-16 (revised, post-review): incorporates codex review. Changes: hot primitives (`element_name` / `attribute` / `text`) added as first-class trait methods, not only `NodeKind` + attrs-slice; `children` split into `dom_children` and `flat_children` to disambiguate DOM-vs-flat-tree traversal Servo distinguishes today; visitor methods use `ControlFlow` so `Stop` actually propagates (the original `Walk::Stop` sketch had a real bug — child returning Stop only returned from its own walk call, parent loop continued); new "foreign trait adapters" section explicitly designs in a `StyleElement<'a, D>` escape hatch for Stylo / `selectors::Element`; "wider applicability" reframed from mandate to candidate house pattern; `StaticDocument` Sync caveat corrected (it's Vec-backed; the `Rc<RefCell<…>>` is only in the parser sink). The framing is now **ID-first core API + traversal helpers**, not "universal visitor religion."
- 2026-05-16 (post-probe): paper probe against `selectors::Element` and `style::dom::TElement` at servo/stylo rev `572ecba2d160`. Confirmed adapter pattern holds. Two trait-shape changes: (1) `prev_sibling` / `next_sibling` added as direct primitives on `LayoutDom` (selector matching can't pay O(siblings) per access); (2) `StyleElement` adapter shape is `(dom, id, style_storage_ref, atom_storage_ref)`, not just `(dom, id)` — the extra state lives in `serval-layout`, not `layout_dom_api`. Computed-style-access open question resolved by the probe.

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

use std::ops::ControlFlow;

pub trait LayoutDom {
    type NodeId: Copy + Eq + Hash + Debug + 'static;

    // --- identity / structure ---

    fn document(&self) -> Self::NodeId;
    fn parent(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Previous sibling in DOM order. Hot on selector matching paths
    /// (`prev_sibling_element` in `selectors::Element`); deriving it from
    /// `dom_children(parent)` would be O(siblings) per call.
    fn prev_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Next sibling in DOM order. See `prev_sibling`.
    fn next_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// DOM-tree children (parse-order, ignores shadow trees).
    fn dom_children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_;

    /// Flat-tree children (slot-assigned for shadow hosts, otherwise DOM order).
    /// Backends without shadow DOM default this to `dom_children`.
    fn flat_children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_ {
        self.dom_children(id)
    }

    // --- kind & hot primitives ---

    fn kind(&self, id: Self::NodeId) -> NodeKind;

    /// Element name when `id` is an element, else `None`. Hot on selector/style paths.
    fn element_name(&self, id: Self::NodeId) -> Option<&QualName>;

    /// Attribute lookup by namespace + local name. Hot on selector/style paths.
    /// Avoid `attrs() -> &[Attribute]` for selector matching — backends may store
    /// attrs in a column store; full-slice exposure forces a materialization.
    fn attribute(&self, id: Self::NodeId, ns: &Namespace, local: &LocalName) -> Option<&str>;

    /// Iterate attributes for serialization / introspection (cold).
    fn attributes(&self, id: Self::NodeId) -> impl Iterator<Item = AttributeView<'_>> + '_;

    /// Text content when `id` is a text or comment node, else `None`.
    fn text(&self, id: Self::NodeId) -> Option<&str>;

    // --- traversal ---

    /// Default walk over the C primitives, descending via `dom_children`.
    /// Backends override if they want backend-driven traversal (parallel layout
    /// pass, prefetching, etc.) or flat-tree-shaped descent.
    fn walk<V: NodeVisitor<Self>>(&self, visitor: &mut V) -> ControlFlow<V::Stop> {
        walk_subtree(self, self.document(), visitor)
    }
}

pub enum NodeKind {
    Document,
    Doctype,
    Element,
    Text,
    Comment,
    ProcessingInstruction,
}

pub struct AttributeView<'a> {
    pub name: &'a QualName,
    pub value: &'a str,
}

pub trait NodeVisitor<D: LayoutDom + ?Sized> {
    /// Early-termination payload. Use `()` when you don't need a typed bail value;
    /// use `core::convert::Infallible` when you can't bail at all.
    type Stop;

    fn enter(&mut self, _dom: &D, _id: D::NodeId) -> ControlFlow<Self::Stop, Descent> {
        ControlFlow::Continue(Descent::Descend)
    }
    fn exit(&mut self, _dom: &D, _id: D::NodeId) -> ControlFlow<Self::Stop> {
        ControlFlow::Continue(())
    }
}

pub enum Descent { Descend, Skip }

pub fn walk_subtree<D, V>(dom: &D, root: D::NodeId, v: &mut V) -> ControlFlow<V::Stop>
where
    D: LayoutDom + ?Sized,
    V: NodeVisitor<D>,
{
    match v.enter(dom, root)? {
        Descent::Skip => ControlFlow::Continue(()),
        Descent::Descend => {
            for child in dom.dom_children(root) {
                walk_subtree(dom, child, v)?;
            }
            v.exit(dom, root)
        }
    }
}
```

Notes on the sketch:

- **Hot primitives first-class.** `element_name`, `attribute`, `text` are direct trait methods, not derived from `NodeKind` + an attrs slice. Selector/style matching reads attributes on the hot path; forcing every match site to call `kind()` and pattern-match before reaching attrs would pessimize. Backends with column-stored attributes can implement `attribute()` as a keyed lookup without materializing a full slice.
- **`NodeKind` is a small enum, not a payload-carrying one.** Callers that want details call the specific accessor (`element_name`, `text`, etc.). Avoids the `NodeKind<'_>` lifetime that the original sketch carried, and keeps the cold-path `attributes` iterator separate from the hot-path `attribute` lookup.
- **`dom_children` and `flat_children` separate.** Servo distinguishes these today (`LayoutNode::dom_children()` vs `flat_tree_children()`). Shadow-DOM-aware layout cares; static profile does not. `flat_children` defaults to `dom_children`; backends without shadow trees pay nothing.
- **`ControlFlow` for traversal.** `NodeVisitor::Stop` is the early-termination type. `ControlFlow<Self::Stop, Descent>` from `enter` means "Break(stop_value)" propagates up through `walk_subtree`'s `?` operator, terminating the whole walk. The original `Walk::Stop` sketch was buggy: returning `Stop` from a child only returned from the child's `walk_subtree`; the parent loop continued. Fixed.
- **Fallibility first-class.** `Self::Stop` is generic, so visitors can carry typed errors out of the walk (`type Stop = SerializationError` for a serializer, `type Stop = Infallible` for a node-counter). Avoids the "now we need a fallible variant" refactor later.
- **`walk` default impl** descends via `dom_children`. Backends that want flat-tree descent override `walk` (not `flat_children`-only — the default needs to be visible to readers as "DOM order").
- The recursion bound is DOM depth. Pathological deeply-nested HTML bottoms out in the default walker. If that becomes a real problem, switch the default to an explicit stack. Out of scope for first cut.

### Foreign trait adapters (the escape hatch)

Stylo's `style::dom::TElement` and `selectors::Element` traits both want **typed element handles** with shape `T: Element { type Impl: SelectorImpl; ... }`. They cannot be expressed against a `(dom, NodeId)` pair directly — they predate the ID-first design and need a handle that carries enough state to satisfy `Copy` plus per-method calls without an extra `dom` argument.

The design **expects** this and provides an adapter pattern. The actual adapter shape (informed by the paper probe — see appendix) carries more state than just `(dom, id)`:

```rust
// In serval-layout (not in layout_dom_api).
pub struct StyleElement<'a, D: LayoutDom> {
    dom: &'a D,
    id: D::NodeId,
    /// Cascaded style data side-table, keyed by NodeId. Layout owns the
    /// cascade output; DOM doesn't carry it. `borrow_data()` reads from here.
    style: &'a StyleStorage<D::NodeId>,
    /// Atom-interned attribute storage for id/class lookups Stylo demands as
    /// `&WeakAtom`. Backed by lazy interning over the DOM's string attrs.
    atoms: &'a AtomStorage<D::NodeId>,
}

impl<'a, D: LayoutDom> selectors::Element for StyleElement<'a, D> {
    type Impl = ServalSelectorImpl;

    fn opaque(&self) -> selectors::OpaqueElement {
        selectors::OpaqueElement::new(&self.id)
    }
    fn parent_element(&self) -> Option<Self> {
        self.dom
            .parent(self.id)
            .map(|pid| self.with_id(pid))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.dom.prev_sibling(self.id).map(|p| self.with_id(p))
    }
    // ... etc.
}
```

Same pattern for `style::dom::TElement` and the rest of the Stylo trait family. Methods that mutate element state (e.g., `apply_selector_flags`, the `unsafe fn set_dirty_descendants`) no-op for the static profile (no incremental restyle); the scripted profile will need real implementations.

**Pattern A as adapter, not as architecture.** This is a feature, not a violation:

- The public `layout_dom_api` surface stays ID-first.
- Stylo / selectors get handle-shaped types they already know how to consume.
- The handle types live in `serval-layout` (or wherever Stylo is consumed), not in `layout_dom_api` — so the DOM crate stays usable by reader-mode, serialization, etc., that don't need Stylo.
- If Stylo's trait shape changes upstream, only `serval-layout`'s adapter changes; the DOM crate doesn't move.

The same escape hatch is available for any other foreign trait that wants typed handles (devtools' inspector protocol, an a11y tree API that demands `aria_*` typed accessors, etc.). The rule: foreign trait wants pattern A → write an adapter struct over `(dom, id, [per-consumer state])`, don't reshape `LayoutDom`.

### Stylo probe results (appendix, 2026-05-16)

A paper probe against `selectors::Element` and `style::dom::TElement` at servo/stylo rev `572ecba2d160` (the workspace pin) confirmed the adapter pattern holds, with three substantive findings:

**Finding 1: sibling primitives are required on `LayoutDom`.** `selectors::Element` exposes `prev_sibling_element` / `next_sibling_element` / `first_element_child` on the hot path. Computing siblings indirectly (`dom.parent(id) → dom.dom_children(parent).find(...)`) is O(siblings); selector matching can't pay that per descendant. Added `prev_sibling` and `next_sibling` as direct primitives on `LayoutDom` (see updated trait sketch above).

**Finding 2: the adapter carries non-trivial side state.** Beyond `(dom, id)`, the Stylo adapter needs:

- A **style storage side-table** keyed by `NodeId`, owned by `serval-layout`. `TElement::borrow_data() -> Option<AtomicRefCell<ElementData>>` reads from here; the cascade writes to it. This stays out of `layout_dom_api` by design — DOM doesn't carry cascade output.
- **Atom-interned id/class storage**, also keyed by `NodeId`. Stylo's `id() -> Option<&WeakAtom>` and `each_class<F>` require atoms, not strings. Interning happens at parse time (eager) or first lookup (lazy); decided at P2.2 implementation time. Also stays out of `layout_dom_api` — the bare DOM stores strings; atom views are a Stylo-consumer concern.

This means the adapter struct is meatier than the original "just `(dom, id)`" framing suggested — it's `StyleElement<'a, D> { dom, id, style: &'a StyleStorage, atoms: &'a AtomStorage }` — but all of the extra state still lives in `serval-layout`, not in `layout_dom_api`. The architectural separation holds.

**Finding 3: stateful trait methods are mostly no-op-friendly for the static profile.** Stylo's incremental restyle protocol (`has_dirty_descendants`, `set_dirty_descendants`, `has_snapshot`, `handled_snapshot`, `apply_selector_flags`) all mutate per-element bits during restyle. Static profile doesn't restyle — it computes style once per layout pass. These methods can no-op (returning `false` for queries, doing nothing for mutators). Trait shape is satisfied; behavior is correct for the static profile. Scripted profile, when it lands, needs real implementations; that's a P4 problem.

**Methods that need real shape decisions before P2.3, not just no-ops:**

- `style_attribute() -> Option<ArcBorrow<Locked<PropertyDeclarationBlock>>>` — element's parsed inline `style="..."`. Static profile parses lazily on first access; needs an LRU or per-element cache. Lives in `serval-layout`, not `layout_dom_api`.
- `each_attr_name<F>` — iterates attribute names as `&LocalName`. The DOM's `attributes(id)` iterator yields `AttributeView<'_>`; the adapter projects this into `LocalName`-shaped callbacks. Mechanical, but needs the `LocalName` type to be reachable from the adapter, which means `serval-layout` depends on `stylo_atoms` (already in workspace deps).
- `state() -> ElementState` — DOM element state bits (hover, focus, disabled, etc.). Static profile: all zero. No interaction means no state.

**Probe verdict:** the adapter pattern is viable. The DOM crate stays Stylo-free; the consumer-side adapter is real implementation work but it's where the work belongs. No architectural retreat. Real prototype (compiling adapter against the actual Stylo trait surface) is deferred to P2.3 when serval-layout actually consumes Stylo — at this stage, the paper probe is enough to confirm we're not designing around an impossible shape.

### Caveats and cost

1. **Less familiar to Servo-derived contributors.** Existing `components/layout/` code reaches for `LayoutNode::parent_node()` directly; the new shape reaches for `dom.parent(id)`. Mental shift is small but real.
2. **Lift cost from pattern-A code is higher than a pattern-A trait would impose.** Estimate: P2.3 takes 10–20% more time than a straight port. Mitigation: the port is batch-by-batch anyway; each batch absorbs the shape change in isolation.
3. **Pattern-A's "this handle is definitely an Element" type safety is lost.** Mitigation: `kind()` returns an enum; `Walk::Skip` lets visitors bail early on non-matching nodes. The matches in traversal code are unavoidable anyway (you're checking node type before doing per-kind work in either pattern).
4. **Random access patterns work fine** because IDs are first-class. `querySelector` returns an `Option<NodeId>`; hit testing returns a `NodeId`; caller does `dom.kind(id)` to dispatch.
5. **`Send + Sync` decisions are pushed down to the impl.** `LayoutDom` doesn't require either; per-backend choice. `StaticDocument` (in `serval-static-dom`) is `Vec<StaticNode>`-backed; the `Rc<RefCell<…>>` in the parser code lives in `StaticTreeSink` and is gone by the time `TreeSink::finish` returns the document. The finished document is `Send + Sync` (all field types are). A future scripted DOM will need to be `Sync` if `LayoutHostServices` keeps its Sync bound, and that's the load-bearing case — the P1 fallout addendum in the strategy doc captures the historical version of that Sync conversation. Deferred to P4.

### Exit criteria — when we'd abandon this and switch to pattern A

If any of these turn out true during P2.3, the pattern isn't paying its costs and we revert to pattern A:

- The match-on-NodeKind sites become a measurable hot path (after profiling, not before).
- The lift cost balloons past 30% extra vs. a pattern-A baseline because too much existing layout code wants direct typed-handle access.
- Stylo's integration points genuinely can't bridge to opaque IDs without a typed-handle adapter shim that's bigger than the trait itself.
- A scripted-DOM provider arrives and its Sync requirements push us into a corner the pattern can't accommodate.

Reversal is straightforward — `layout_dom_api` is a young crate, callers are few, the trait surface is small. The cost is rewriting `serval-static-dom`'s impl and any `serval-layout` callsites that consumed the trait. Bisect-friendly history means we can identify and back out cleanly.

---

## Wider applicability — candidate house pattern (informational)

If this pattern carries its weight in `layout_dom_api`, it becomes a **candidate** house pattern for owned tree/graph APIs in the ecosystem. **Not a mandate.** Each candidate site has its own shape, and the fit varies:

- *serval-layout's fragment tree* — strong fit. IDs for hit-testing, visitor for paint emission. Same identity-vs-walk split as DOM.
- *mere/graphshell graph crate* — strong fit. Already ID-keyed in spirit (NodeIndex/EdgeIndex). Visitor pattern matches petgraph's prior art, which the graph crate effectively already follows.
- *eidetic's content store* — strong fit. Content addressable by hash/ID already; a visitor for "walk all stored content of kind X" would be streamable, paged, async-friendly.
- *mere's panel registry* — moderate fit. Already ID-based for identity (panel summons by `PanelId`). Whether a visitor adds value depends on whether "walk all panels" is a real operation; if the registry is mostly point-lookups, just stay ID-keyed without a visitor.
- *netrender's display list* — **uncertain fit, depends on shape.** If the display list is tree-shaped or resource-ID-shaped (clip chains, reference frames, stacking contexts as addressable entities), the pattern fits. If it's a compact command stream (a `Vec<DisplayItem>` walked linearly without IDs into specific items), pattern B alone — or even no formal pattern at all, just iterate the slice — fits better than the hybrid. Decide when netrender's internal display-list rewrite stabilizes; don't force the pattern there speculatively.

The candidate decision rule, when introducing a new tree/graph-shaped API:

1. Are identity operations a real part of the surface? If yes, opaque IDs.
2. Is there a dominant "walk all" mode? If yes, visitor with default impl over the IDs.
3. Are foreign traits in the consumer set (Stylo-shaped libraries that want typed handles)? If yes, write adapters at the consumer; don't reshape the core API.
4. Is the data shape actually a compact command stream / iterator-y? Then the pattern doesn't apply; just expose a slice or iterator.

Validate `layout_dom_api` first. If it pays its costs (10–20% extra lift, plus the foreign-adapter overhead), propose extending to fragment tree next; then judge each subsequent crate on its own shape.

---

## Open questions

Resolved in the revision pass:

- ~~NodeKind shape (payload-carrying vs. plain enum + accessors).~~ Resolved: plain enum, hot accessors (`element_name`, `attribute`, `text`) are first-class trait methods, cold `attributes()` iterator separate.
- ~~Attribute lookup primitive vs. derived from slice.~~ Resolved: primitive (`attribute(id, ns, local)`), with `attributes()` iterator for cold serialization/introspection paths.
- ~~Where the default `walk` impl lives.~~ Resolved: default trait method, with `walk_subtree` exposed as `pub fn` for callers that want explicit subtree walking.
- ~~Traversal flavor (single `children` vs. `dom_children` + `flat_children`).~~ Resolved: split, with `flat_children` defaulting to `dom_children` so backends without shadow trees pay nothing.
- ~~Visitor early-termination shape.~~ Resolved: `ControlFlow<Self::Stop, Descent>`. Carries typed bail values; fixes the `Walk::Stop` propagation bug.
- ~~Foreign trait integration (Stylo, selectors).~~ Resolved: adapter struct `StyleElement<'a, D> { dom, id }` in `serval-layout`, implementing `selectors::Element` / `TElement` over the ID-keyed core API. Pattern A as escape hatch, not architecture.

Still open:

1. **Mutation surface.** This sketch is read-only. Scripted DOM needs mutation (innerHTML, appendChild, etc.). Decide later whether mutation goes in a `LayoutDomMut: LayoutDom` extension trait or in a separate trait. Static profile doesn't care; defer to when scripted lands.
2. **Crate name spelling.** `layout-dom-api` is the working choice (matches `layout_api`, `paint_api`, `script_traits` precedent — package name hyphenated, Rust import underscored). Confirm at scaffold time.
3. **Pseudo-elements / shadow / template traversal.** Beyond `dom_children` and `flat_children`, fullweb layout cares about `::before` / `::after` synthetics, shadow trees, template contents. Add these as named flavors (`pseudo_children`, `shadow_children`) when the first non-static profile needs them, not now. Static profile doesn't have any of these.
4. ~~Computed-style access.~~ Resolved by the 2026-05-16 Stylo probe (see appendix): side-table in `serval-layout`, keyed by `NodeId`, not a `LayoutDom` primitive. `StyleElement` adapter carries `&'a StyleStorage<NodeId>` reference. Same pattern for atom-interned id/class storage. Keeps `layout_dom_api` Stylo-free; consumers that don't care about style (reader-mode, serialization) don't pay for it.

---

## Review checklist

Codex review (2026-05-16) addressed; the revisions above incorporate every actionable point. Items still in this checklist are for any further reviewer:

- [x] Is the foreign-trait-adapter escape hatch genuinely sufficient for Stylo's `TElement`, or are there Stylo trait methods (e.g., `pseudo_element_originating_element`) that demand state the `(dom, id)` adapter can't carry? **Answered by the 2026-05-16 paper probe** (see appendix). Adapter holds; carries more state than the original sketch implied (style storage + atom storage references), all in `serval-layout`. Real compiling prototype deferred to P2.3 when Stylo is actually consumed.
- [ ] Is the recursion-bound walk default acceptable, or should we ship the explicit-stack version from day one? Pathological deeply-nested HTML can blow stack; mitigation is feasible later, but easier now if the visitor surface stays simple.
- [ ] Are the exit criteria (when we'd revert to pattern A) tight enough that we'd actually notice and act? Currently: NodeKind match becomes measured hot path, lift cost >30% over pattern-A baseline, Stylo can't bridge, or scripted-DOM Sync pushes us into a corner. Add more if there are blind spots.
- [ ] Is netrender's display list shape known well enough today to predict whether the candidate pattern applies there? If not, hold off on declaring fit; revisit when the netrender internal rewrite stabilizes.
- [ ] Should `layout_dom_api` ship with a minimal `serval-static-dom` impl alongside (to validate the trait against a real backing store) or land empty and have `serval-static-dom`'s impl follow in the next commit? Lean toward "ship impl together" — an unvalidated trait is a guess.
