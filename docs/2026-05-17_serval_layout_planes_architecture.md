# serval-layout architecture: planes (proposed, for review)

**Status (2026-05-17, revised PM):** proposed. Resolves the "Blitz vs path-C lift" question raised after reading `linebender/blitz` and `DioxusLabs/blitz`, by synthesizing both with the goals path C was protecting.

**Companion doc (read together):** [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md) — the cross-engine architecture. Serval is **one lane** in Hekate's lane system; Nematic is a peer lane; extract is Hekate's own work, not a Serval head. This doc covers serval-layout's piece (Style + Layout + Fragment + Paint planes for HTML) and how it publishes observables to the host. The lane decomposition and observable-plane vocabulary live in the Hekate doc.

This doc is the architectural reference for `serval-layout`. The implementation plan that follows it is in [2026-05-16_serval_layout_lift_plan.md](./2026-05-16_serval_layout_lift_plan.md) (updated 2026-05-17 to align with this doc).

---

## Context

Three pieces of prior work feed this:

- **Path C lift plan** ([2026-05-16](./2026-05-16_serval_layout_lift_plan.md)) committed to lifting Servo's layout machinery into `serval-layout`. Concrete benefit: preserve years of CSS-spec work. Concrete cost: ~30k lines of layout code to port, plus the `layout_api` LayoutNode/Element/DangerousStyleNode/DangerousStyleElement scaffolding (~1700 lines of Stylo-shaped trait surface).
- **LayoutDom design pass** ([2026-05-16](./2026-05-16_layout_dom_api_design.md)) established the DOM-side trait as ID-first (opaque `NodeId`, visitor with `ControlFlow<Stop, Descent>`, no per-kind handle types, foreign-trait adapters live consumer-side). The DOM crate (`layout_dom_api`) stays Stylo-free; the bridge belongs in `serval-layout`.
- **Blitz read** (2026-05-17). Blitz does **not** port Servo's layout. They use **Stylo for cascade + Taffy for layout + custom glue** (~3k lines in `blitz-dom/src/layout/`). Their `BlitzNode<'a> = &'a Node` collapses Servo's four-type bundle (LayoutNode / LayoutElement / DangerousStyleNode / DangerousStyleElement) to a single Rust reference. No `layout_api`.

Each prior work has a piece of the right answer. The synthesis below takes them in combination.

---

## The architecture: planes

`serval-layout`'s mutable rendering state lives in **planes**, not embedded on DOM nodes. Each plane is keyed by the DOM's opaque `D::NodeId` and owned by `serval-layout`:

| Plane | Owns | Produced by | Consumed by |
| --- | --- | --- | --- |
| **DOM** | identity, structure, attrs, text | — (external; provided by DOM crate) | every other plane |
| **Style** | computed style, atomized id/class, selector flags | Stylo cascade | layout, fragment, paint, a11y, query |
| **Layout** | box tree, formatting-context state, Taffy nodes | construct + Taffy | fragment |
| **Fragment** | rect-per-node, line boxes, hit-test geometry | post-Taffy walk | paint, hit-test, a11y, getBoundingClientRect |
| **Paint** | (ephemeral) `ServalDisplayList` | display-list emission | NetRender |

The DOM plane is supplied by the consumer (`serval-static-dom`, future scripted DOM, reader-mode DOM, etc.) and lives outside `serval-layout`. Every other plane is `serval-layout`'s problem.

### Why planes (and not embed-on-node)

Blitz embeds layout state on `Node`. That's clean **because they have one DOM type**. Their `Node` is a "rendering DOM" — it owns parser tree + Stylo data + selector flags + layout/paint children. Beautifully direct, but it locks the DOM to one role.

Serval explicitly wants multiple DOM providers:

- **Static** (`serval-static-dom::StaticDocument`, today) — parsed once, never mutated; minimal.
- **Scripted** (future) — JS-driven mutations, GC-shaped reflectors, Servo's script DOM shape.
- **Reader-mode** (eventually, smolweb-extract head of three-head Hekate) — readability-extracted content, possibly synthetic.
- **Possibly graphshell / eidetic adjacency** — content stores that might want layout for "render this stored content" purposes.

Embedding layout state on the DOM either forces every provider to carry state it doesn't use, or makes the DOM a layout-aware rendering DOM (which defeats the multi-provider goal). Planes solve this by keeping the DOM clean and putting mutable state where it belongs: in `serval-layout`.

This is also the established pattern in adjacent fields (see [Prior art](#prior-art)) — it's only unusual *for browser layout engines*, where the historical norm is embed-on-element. We have a structural reason to differ.

---

## The handle: `NodeRef<'a, D>`

The unit of address inside `serval-layout` is:

```rust
pub struct NodeRef<'a, D: LayoutDom> {
    dom: &'a D,
    id: D::NodeId,
}
```

(Renamed from `LayoutDomAdapter` per Mark's 2026-05-17 sketch — shorter, more direct.)

`NodeRef` implements Stylo's trait family directly:

- `style::dom::{NodeInfo, TNode, TElement, TDocument, TShadowRoot, AttributeProvider}`
- `selectors::Element<Impl = SelectorImpl>`
- `Hash`, `Eq`, `Copy`, `Debug`, `Send`, `Sync`

These are the **only** places in `serval-layout` that name Stylo's trait surface. A grep canary backstops this:

```powershell
# Should match only inside serval-layout/src/stylo_adapter/
rg -l "style::dom::T(Node|Element)|selectors::Element|AttributeProvider" components/serval-layout/
```

For Stylo's data-storage methods (`borrow_data`, `id`, `each_class`, etc.), `NodeRef` reads/writes the **StylePlane**:

```rust
// Inside the adapter, simplified:
fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
    self.style_plane().borrow(self.id)
}
```

The DOM is just identity + structure; the style state lives in the plane.

### Why `NodeRef` not `LayoutDomAdapter`

Same shape as Blitz's `BlitzNode<'a> = &'a Node`, but parameterized over `D: LayoutDom`. Mark's renaming captures *what it is* (a node reference) rather than *what it bridges* (Blitz's name choice too). Leaves room for typed views (`ElementRef`, `DocumentRef`) later without renaming the default.

---

## Capability traits — what DOMs offer to layout

`LayoutDom` is the minimum: identity, structure, attributes, text. The static profile's `StaticDocument` implements this and nothing else.

Layout needs more for HTML rendering (`<img>` sizing, eventually `<input>` form controls, `<iframe>` browsing contexts, etc.). These live in **capability traits** that DOMs opt into:

```rust
// layout_dom_api (or sibling crate):

pub trait LayoutDom { /* identity + structure + attrs + text */ }

/// Required by layout pipelines (block, flex, etc.). Static profile implements
/// with always-None bodies for image/video — <img> renders as a zero-sized
/// box, but layout works.
pub trait ReplacedElementProvider: LayoutDom {
    fn intrinsic_size(&self, id: Self::NodeId) -> Option<IntrinsicSize>;
    fn image_data(&self, id: Self::NodeId) -> Option<ImageDataRef<'_>>;
    // video, canvas-as-replaced, iframe-as-replaced
}

/// Required by fullweb. Static doesn't need it; layout doesn't need it.
pub trait FormControlProvider: LayoutDom {
    fn input_value(&self, id: Self::NodeId) -> Option<&str>;
    fn input_kind(&self, id: Self::NodeId) -> Option<InputKind>;
}

/// Required by fullweb. Scripted profile implements this when iframes light up.
pub trait EmbeddedContentProvider: LayoutDom {
    fn iframe_browsing_context(&self, id: Self::NodeId) -> Option<BrowsingContextId>;
}
```

Pipeline signatures express their actual needs:

```rust
fn layout<D: LayoutDom + ReplacedElementProvider>(dom: &D, ...) -> LayoutPlane;
fn extract_readability<D: LayoutDom>(dom: &D) -> ReadableContent;  // extract head; no replaced needed
fn fullweb_layout<D: LayoutDom + ReplacedElementProvider + FormControlProvider + EmbeddedContentProvider>(...)
```

**`canvas_data`, `iframe_pipeline_id`, media handles, etc. never go on `LayoutDom`.** That's the firewall.

---

## Plane lifecycle + concurrency

Each plane has a distinct access pattern. Design the storage type up front so we don't retrofit later.

| Plane | Build phase | Concurrency during build | Storage shape | Read concurrency |
| --- | --- | --- | --- | --- |
| **Style** | cascade pass | Stylo's rayon-parallel cascade writes per-node | `IndexVec<NodeId, AtomicRefCell<StyleData>>` (dense) or `FxHashMap<NodeId, AtomicRefCell<StyleData>>` (sparse) | many readers via `borrow()` |
| **Layout** | Taffy run | single-threaded (Taffy doesn't parallelize within a tree) | `IndexVec<NodeId, LayoutNodeState>` with `&mut` access | not concurrent until layout is done; then read-only |
| **Fragment** | post-Taffy walk, write-once | single-threaded write | `IndexVec<NodeId, Fragment>` | many readers (paint, hit-test, a11y, getBoundingClientRect) |
| **Paint** | ephemeral | builder pattern, no plane storage | — (a `ServalDisplayList` builder is passed in) | — (output goes to NetRender) |

The granular concurrency model lets us pick the right cell types:

- `AtomicRefCell` only where Stylo demands it. (Blitz uses `UnsafeCell<Option<ElementDataWrapper>>` everywhere — broader than necessary; a contained correctness risk.)
- Plain `&mut` where the pass is single-threaded.
- Plain `&` once the plane is frozen.

### Plane invalidation (designed in, deferred for static)

Static profile rebuilds all planes from scratch each render. Fullweb needs incremental invalidation:

```text
attribute change → invalidate(StylePlane subtree)
                 → invalidate(LayoutPlane subtree)
                 → invalidate(FragmentPlane subtree)
                 → re-emit Paint
```

Each plane gets two entry points:

```rust
impl StylePlane {
    pub fn rebuild_all<D: LayoutDom>(&mut self, dom: &D, stylist: &Stylist);
    pub fn invalidate(&mut self, id: D::NodeId) { todo!("incremental restyle") }
}
```

The `invalidate` stubs panic for the static profile but exist in the API shape. Fullweb fills them in without re-architecting.

---

## `NodeIdSpace` — dense vs sparse storage

DOM providers have different `NodeId` shapes:

- **`serval-static-dom::StaticNodeId(usize)`** — sequentially assigned by html5ever's tree builder. Dense 0..N.
- **Future scripted-DOM** — likely pointer-shaped or generational. Sparse.

Plane storage adapts:

```rust
pub trait NodeIdSpace: LayoutDom {
    /// Hint: do node IDs form a dense 0..N range?
    fn is_dense() -> bool { false }
    /// Total node count if dense.
    fn node_count(&self) -> Option<usize> { None }
}

// StylePlane / LayoutPlane / FragmentPlane each pick storage:
//   D::is_dense() && D::node_count() known → IndexVec<NodeId, T>
//   otherwise                              → FxHashMap<NodeId, T>
```

Dense storage is nearly as fast as embedding fields on the DOM node — same cache locality, no hash overhead, no allocation per insert. Mark's point about "almost as fast as embedding on Node" rests here.

The fallback for sparse providers is HashMap with `FxHasher` (fast on integer-shaped keys). The cost is real but bounded, and only sparse providers pay it.

---

## Adapter as foreign-trait firewall

The Stylo trait surface is large, gnarly, and Servo-shaped. We contain it.

**Rule:** `style::dom::*`, `selectors::Element`, `AttributeProvider`, `style::data::*`, `stylo_dom::ElementState`, and similar foreign-trait types are named **only inside `serval-layout/src/stylo_adapter/`**.

Everywhere else in `serval-layout`:

- Reads computed style via `StylePlane::primary_style(id)` → `&ComputedValues` (Stylo *value* types, not Stylo *trait* types — the values are unavoidable; the traits aren't).
- Reads selector flags via `StylePlane::selector_flags(id)`.
- Reads element-data via `StylePlane::borrow_data(id)` — wrapper that hides the `ElementDataRef<'_>` shape.
- Never names `TElement`, `TNode`, `selectors::Element`, or related.

This means:

- The adapter's churn (when Stylo upgrades change trait signatures) is bounded to one module.
- The rest of `serval-layout` reads via plane APIs that we control.
- `layout_dom_api` never learns about Stylo at all.
- `serval-static-dom` never learns about Stylo at all.

### Module layout inside `serval-layout`

```text
components/serval-layout/
├── Cargo.toml
├── lib.rs                        — public surface: layout::<D>(dom, viewport) -> LaidOutDoc
├── stylo_adapter/                — the firewall
│   ├── mod.rs                    — NodeRef + structural methods
│   ├── node_info.rs              — NodeInfo + TNode + TDocument + TShadowRoot
│   ├── element.rs                — TElement + selectors::Element + AttributeProvider
│   └── style_storage.rs          — StylePlane access from inside adapter methods
├── style/
│   ├── mod.rs                    — cascade entry: run Stylo over adapter, populate StylePlane
│   └── plane.rs                  — StylePlane definition + storage choice
├── construct.rs                  — DOM walk → Taffy tree construction
├── taffy_glue.rs                 — impl LayoutPartialTree + TraversePartialTree
├── layout/
│   ├── mod.rs                    — run Taffy, populate LayoutPlane
│   └── plane.rs                  — LayoutPlane definition
├── inline/
│   ├── mod.rs                    — inline-context construction
│   ├── measure.rs                — Taffy measure_function delegating to TextMeasure trait
│   └── parley_impl.rs            — TextMeasure impl backed by parley
├── fragment/
│   ├── mod.rs                    — post-Taffy walk, populate FragmentPlane
│   ├── plane.rs                  — FragmentPlane definition (internal storage)
│   └── query.rs                  — FragmentQuery impl (the public ABI; see "Publishing
│                                    observables" below)
├── display_list/                 — lifted from Servo, adapted to read FragmentPlane
│   ├── mod.rs
│   ├── stacking_context.rs
│   ├── background.rs
│   ├── border.rs
│   ├── text.rs                   — parley glyphs → ServalDisplayItem::Text
│   └── hit_test.rs
├── extract.rs                    — Serval's impl of Hekate's ExtractCapableLane trait:
│                                    extract_structure (E1), extract_with_style (E3),
│                                    extract_with_layout (E4). NOT a "three-head extract head";
│                                    Hekate owns extract, Serval cooperates.
├── geom.rs                       — lifted: WM-aware Logical/Physical geometry
├── style_ext.rs                  — lifted: ComputedValuesExt helpers
├── lists.rs                      — lifted: list markers
├── quotes.rs                     — lifted: CSS quote handling
└── replaced.rs                   — lifted partial: intrinsic sizing for img/video
```

---

## Where serval-layout fits in Hekate's lane system

**Correction (2026-05-17 PM):** earlier framing of "three Hekate heads served by serval-layout" was a category error. The right shape (see [Hekate doc](./2026-05-17_hekate_lanes_observables.md)):

- Hekate is the **router + document-intelligence layer**. Not a renderer. Owns source sniffing, capability detection, route choice, extract tiers, observables cache.
- **Nematic** is a peer engine lane for protocol-faithful smolweb sources (Gemini, Scroll, Markdown, feeds). It does not route through HTML.
- **Serval** is the HTML/CSS/(JS) lane. Two profile facades wrap `serval-layout`: `serval-static-html` (Middlenet, no JS) and `serval-fullweb` (full browser).
- **Extract** is Hekate's own work. Tiers E0–E2 happen in Hekate. E3 (style-assisted) and E4 (layout-assisted) escalate **into** the lane via `ExtractCapableLane::extract_with_style` / `extract_with_layout` — Hekate doesn't run Stylo or Taffy itself.

What this means concretely for `serval-layout`:

- The "extract head" is **not** a Serval feature. `serval-layout/src/extract.rs` is Serval's *implementation* of Hekate's `ExtractCapableLane` trait — Hekate asks "extract style-assisted facts from this document," Serval runs the cascade and returns observables Hekate caches.
- The middlenet and fullweb profile facades both use `serval-layout`'s planes. Middlenet doesn't build invalidation; fullweb does.
- Serval lives as one of several lanes; it doesn't know about Hekate's routing decisions or other lanes. The host (mere) chooses lanes via Hekate; lanes just publish observables.

---

## Publishing observables: the FragmentQuery + InteractionQuery surface

Per Mark's correction: **don't expose raw layout internals as a permanent ABI.** Internal plane storage (IndexVec, FxHashMap, the `Fragment` struct shape, line-box representation) is implementation detail and should evolve freely. Consumers (apparatus, host, scroll-to-anchor, selection highlight, `getBoundingClientRect` when scripted lands) speak a query-surface trait.

`serval-layout` implements the cross-engine `FragmentQuery` trait (defined in the engine-observables crate; see [Hekate doc](./2026-05-17_hekate_lanes_observables.md)) over its internal FragmentPlane + StylePlane data:

```rust
// In serval-layout/src/fragment/query.rs:
impl<D: LayoutDom> FragmentQuery for LaidOutDoc<'_, D> {
    type FragmentId = ServalFragmentId;

    fn generation_id(&self) -> u64 { self.epoch }

    fn hit_test(&self, point: Point) -> Option<FragmentHit> {
        self.fragment_plane.hit_test_at(point, &self.style_plane)
    }

    fn box_model(&self, source_id: SourceNodeId) -> Option<BoxModel> {
        let fragment = self.fragment_plane.fragment_for_source(source_id)?;
        Some(self.fragment_plane.box_model_of(fragment, &self.style_plane))
    }

    fn fragments_for_anchor(&self, anchor: &str)
        -> Box<dyn Iterator<Item = Self::FragmentId> + '_>
    {
        self.fragment_plane.fragments_for_anchor(anchor)
    }

    fn text_range_for_fragment(&self, fragment: Self::FragmentId)
        -> Option<SourceRange>
    {
        self.fragment_plane.source_range(fragment)
    }

    fn rects_for_selection(&self, range: SourceRange) -> Vec<Rect> {
        self.fragment_plane.selection_rects(range)
    }
}
```

The plane structs (`StylePlane`, `LayoutPlane`, `FragmentPlane`) stay `pub(crate)` inside `serval-layout`. The public surface is the trait impls.

Same pattern for `InteractionQuery` (focus, selection, affordances, activation targets) — `serval-layout`'s impl reads StylePlane + FragmentPlane internally; the public ABI is the trait.

---

## Accessibility as fusion (not "from FragmentPlane alone")

Per Mark's correction: **a11y is a fusion of three planes**, not built from any one of them.

- **Source/Semantic Plane** (= DOM via `LayoutDom` view) gives names, roles (`aria-role`, semantic-element-implied roles), language, ARIA relationships, source spans for citation-back-to-source.
- **Style Plane** gives computed visibility (`visibility`, `display`, `aria-hidden` interaction), language inheritance, computed text direction.
- **Fragment Plane** gives geometry (for "where on screen is this thing"), visibility evidence (offscreen fragments are functionally hidden even if CSS-visible), and reading order via fragment traversal.

The a11y tree builder lives in `mere/apparatus/` (or wherever mere owns the cross-engine accessibility composition), not in `serval-layout`. It queries the three planes via the trait API. The same builder works for any lane that publishes the three planes (Serval HTML pages, Nematic Gemini pages, etc.) — Nematic publishes Source/Semantic + Fragment but no Style; the builder degrades gracefully.

This means `accessibility_tree.rs` from Servo (410 lines) **doesn't get lifted into serval-layout**. The cross-engine a11y composition is mere's job. Serval's contribution is publishing the three planes correctly.

---

## Prior art

This pattern is unusual for browser layout engines (Servo, Blitz, WebKit, Gecko all embed on element/node), but well-established adjacent:

- **rustc query system** — queries keyed by `DefId`, stored in side-tables. Salsa-style incremental invalidation. Heavy machinery but identical principle.
- **Salsa** — incremental computation framework. Query-keyed; invalidation graph computed from query dependencies. Closest match for the eventual fullweb invalidation.
- **Bevy / Hecs / specs (Rust ECS)** — entity-component-system. World owns components keyed by Entity. Dense + sparse storage choices match what we'd do per plane.
- **WGPU resource handles** — `BufferId` / `TextureId` indexing into device-owned tables.
- **petgraph** — graph traversal over opaque NodeIndex. (Already cited as prior art for `LayoutDom`'s ID-first design.)

The novelty for us isn't risk — it's applying a known good pattern in a domain where the historical norm doesn't fit our multi-DOM-provider goal.

---

## What this means for the path-C lift list

The lift list shrinks substantially under the planes architecture. From [the path-C plan's file-by-file triage](./2026-05-16_serval_layout_lift_plan.md):

### Lift now (static profile load-bearing)

| File / dir | Lines | Role under planes |
| --- | --- | --- |
| `display_list/` | ~3.4k | Adapted to read FragmentPlane + StylePlane instead of Servo's fragment_tree. The Servo emission machinery (stacking-context, background, gradient, clip, hit_test) is ~80% structural and translates. |
| `style_ext.rs` | 1319 | `ComputedValuesExt` helpers used by display_list/ and inline/. Pure Stylo-helper code; no DOM coupling. |
| `geom.rs` (selective ~300/692) | | WM-aware Logical/Physical geometry. Taffy has plain geometry but doesn't model logical axes; lift the WM bits. Skip the `ContainingBlock`-wrapping parts (don't need with Taffy). |
| `lists.rs` | 164 | CSS list markers; Taffy doesn't do this. Small. |
| `quotes.rs` | 427 | CSS `content: open-quote / close-quote / counter()`. Edge but cuttable later. |
| `replaced.rs` (partial ~300/775) | | Intrinsic sizing + aspect-ratio resolution. Fed into Taffy via `measure_function`. Skip the canvas/iframe wiring. |

**Lifted total: ~5.5k lines.**

### Lift later (fullweb profile only)

| File / dir | Why deferred |
| --- | --- |
| `flow/float.rs` (1147) | CSS `float`. Taffy doesn't support floats. Legacy CSS; defer. |
| `formatting_contexts.rs` (605) | Fine-grained BFC/IFC control. Taffy covers the common case implicitly. |
| `traversal.rs` (289) | Incremental restyle. Plane `invalidate` lights up here. |
| `positioned.rs` (980) | Stacking-context handling beyond Taffy's positioning. Probe Taffy first. |
| `query.rs` (1558) | Layout-time DOM queries (`getBoundingClientRect`). Reads FragmentPlane when scripted lands. |

### Drop entirely (~30k lines)

`flexbox/`, `flow/mod.rs`, `flow/construct.rs`, `flow/inline/*`, `dom.rs`, `dom_traversal.rs`, `construct_modern.rs`, `layout_box_base.rs`, `layout_impl.rs`, `accessibility_tree.rs`, `sizing.rs`. Replaced by Taffy + parley + our plane construction code.

### New code (~3-4k lines)

- `stylo_adapter/` — NodeRef + Stylo trait impls (≈ Blitz's `stylo.rs`, structurally; ~1000 lines).
- `style/` — cascade entry + StylePlane storage (~500 lines).
- `construct.rs` — DOM walk → Taffy tree (~500 lines).
- `taffy_glue.rs` — Taffy trait impls over our context (~300 lines).
- `inline/` — parley wiring + measure function (~500 lines).
- `fragment/` — post-Taffy walk → FragmentPlane (~300 lines).
- `extract.rs` — three-head Hekate extract head (~200 lines, deferred).

**Net total estimate: ~8-10k lines** in `serval-layout` (lifted + new). A quarter of path-C's original 30k-line lift, while preserving the spec-compliance lifts we actually want and getting Taffy as the load-bearing layout algorithm.

---

## Open questions for review

1. **Capability-trait crate location.** `ReplacedElementProvider` / `FormControlProvider` / `EmbeddedContentProvider` live in `layout_dom_api`? Or in a sibling crate `layout_capabilities_api`? Or inside `serval-layout`? Argument for separate: capability traits are forward-leaning; readers that consume `LayoutDom` (extract head, serialization) shouldn't be forced through them. Argument for `layout_dom_api`: one crate for "the DOM-side contract." Lean separate; named e.g. `layout_capabilities_api`.

2. **Plane visibility.** Are the planes public read-only API of `serval-layout` (so mere/apparatus/a11y can read FragmentPlane directly), or `pub(crate)` with controlled accessor methods? Lean public-read — the "planes as reusable observables" framing depends on it.

3. **StylePlane storage primitive.** `AtomicRefCell<StyleData>` or `RwLock<StyleData>` or something else? Stylo's parallel cascade writes one node per thread; `AtomicRefCell` is the cheapest "checked at runtime" option. Lean `AtomicRefCell`; revisit if profiling shows contention.

4. **Taffy spec coverage validation.** I've claimed Taffy covers block + flex + grid well enough for the static profile. Need a probe to verify edge cases (writing-mode, baseline alignment, sticky positioning, intrinsic sizing edge cases). The probe slice (next session) should test enough to catch big gaps.

5. **NodeIdSpace ergonomics.** Adding `is_dense()` + `node_count()` to `LayoutDom` (or as a separate trait) imposes implementation cost on every DOM. Could default to `false` / `None` and let backends opt in. Decide at adapter-write time.

6. **Display-list lift vs rewrite.** I claim `display_list/` is ~80% structural and lifts cleanly. The adaptation cost is "feed it FragmentPlane instead of Servo's fragment_tree." This could be larger than the framing implies — Servo's `display_list/` consumes fragment-tree-shaped intermediate types (Fragment, BoxFragment, etc.) that don't exist under planes. Either: (a) define equivalent types in `serval-layout::fragment` that match Servo's shape (allowing high lift fidelity), or (b) refactor the lifted display-list code to consume plane data directly. Lean (a) — preserves lift value; the cost of equivalent types is small.

---

## Review checklist

For Mark or codex or whoever reviews next:

- [ ] Is "planes" the right level of abstraction, or are we over-decomposing? (Counter-argument: collapse Style + Layout + Fragment into a single "RenderState" with submodules. Loses the per-plane concurrency clarity but is simpler.)
- [ ] Is the capability-trait split right (ReplacedElementProvider / FormControlProvider / EmbeddedContentProvider), or do we need different categories? (Alternative: one "FullDom" trait that bundles everything fullweb needs.)
- [ ] Does the FragmentPlane-as-reusable-observable framing actually pay off for mere/apparatus, or is it speculation? (Worth checking with mere's roadmap if we're committing to it.)
- [ ] Are the "lift now" / "lift later" / "drop" categories right? In particular: should `accessibility_tree.rs` be lifted now (path C kept it as W3C goods) or rebuilt from FragmentPlane+StylePlane later (this doc's framing)?
- [ ] Is the foreign-trait firewall rule "Stylo trait names only inside stylo_adapter/" tight enough? Or should the rule be tighter (e.g., "no Stylo *types* — including ComputedValues — outside named plane accessor APIs")? Lean less tight: ComputedValues is a *value* type, hard to avoid in style-aware code.
- [ ] Probe scope for the next session: what's the smallest end-to-end that validates this? I propose: NodeRef + minimal Stylo adapter + StylePlane skeleton + tiny construct.rs that builds a Taffy tree for one `<p>` + run Taffy + log the resulting rect. ~500 lines. Renders nothing; proves the wiring.

---

## Decision log

- **Decided 2026-05-17:** Adopt planes architecture. Resolves the "Blitz vs path-C lift" framing by taking Blitz's adapter collapse + Taffy + parley, with planes for serval-layout-owned mutable state instead of Blitz's embed-on-Node. Multi-DOM-provider goal preserved.
- **Renamed 2026-05-17:** `LayoutDomAdapter` → `NodeRef`. (Existing `LayoutDomAdapter` code stays under that name until next rewrite touches it; doc uses new name.)
- **Deferred:** plane invalidation (`invalidate(id)`) is stubbed for static profile. Fills in when fullweb invalidation lights up.
- **Open:** capability-trait crate location (lean separate `layout_capabilities_api`); plane visibility (lean `pub` reads); display-list lift mechanism (lean (a) equivalent fragment types in `serval-layout::fragment`).
