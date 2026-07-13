# Box-tree (taffy trait-impl tree) — retire `cv_to_taffy`, zero-copy style

Status: **DONE (2026-05-25)**. Follow-on to
[2026-05-20_stylo_taffy_adoption_plan.md](./2026-05-20_stylo_taffy_adoption_plan.md),
which closed the converter side but left `cv_to_taffy.rs` undeletable
under the owned-`Style` `TaffyTree` model. This plan removed that model —
the box tree is now genet's layout engine and `cv_to_taffy.rs` is
deleted. See [Outcome](#outcome-2026-05-25).

## Goal (what "done" buys us)

The stylo_taffy adoption left one done-condition unmet for a real reason:
`TaffyTree<NodeContext = ()>` is **not generic over the custom-ident
type**, so it can only store `Style<DefaultCheapStr>`. To put a node's
style in the tree, genet must *build* a `taffy::Style` from
`ComputedValues` — which is exactly what `cv_to_taffy::to_taffy_style`
is. The file can't be deleted while `TaffyTree` is the arena.

taffy's **trait-impl tree** is the way out: instead of storing `Style`
in taffy's arena, genet owns its own box-tree arena and implements
taffy's traversal + style-access traits. The style accessor returns
`stylo_taffy::TaffyStyloStyle` — a **zero-copy** wrapper that reads
layout properties straight off `ComputedValues`. taffy's layout
algorithms stay in taffy (we call `compute_block_layout` etc.); we
supply only the tree shape + style access.

Wins: (1) `cv_to_taffy.rs` deletes; (2) no per-node `Style` rebuild on
every layout — the cascade's `Arc<ComputedValues>` is read directly;
(3) named grid lines become reachable (`Atom` ident flows through);
(4) the tree has the same shape as blitz-dom, so future technique
borrows are cheaper.

## The key simplification (verified)

The naive worry was a `BoxStyle` enum (`Stylo(..)` vs an owned default
for synthetic/text nodes) implementing all 7 taffy style traits by 2-arm
delegation — ~250 LOC of boilerplate across 52 methods. **Not needed.**
Every node can carry an owned `servo_arc::Arc<ComputedValues>`, so the
style GAT is uniformly `TaffyStyloStyle<ServoArc<ComputedValues>>` — one
type, no enum, no delegation. Confirmed facts that make this work:

- `ElementStyles::primary() -> &Arc<ComputedValues>`
  (`stylo` `style/data.rs:189`). Clone is a cheap refcount bump, not a
  deep copy — still "zero-copy" in the sense that matters (no `Style`
  rebuild, no property-by-property copy).
- `servo_arc::Arc<ComputedValues>: Deref<Target = ComputedValues>`, so
  it satisfies `TaffyStyloStyle<T: Deref<Target = ComputedValues>>`.
  Passing the `Arc` *by value* sidesteps the borrow-lifetime problem of
  returning `&ComputedValues` from a temporary `ElementDataRef` guard
  (the reason a borrowed GAT wouldn't compile).
- **No synthetic root.** `compute_root_layout(tree, root, available)`
  resolves the root's `size()` against `available_space` (taffy
  `compute/mod.rs:64`). `<html>` carries UA `width:100%; height:100%`,
  so making `<html>` the real root resolves to the viewport directly —
  the synthetic viewport-sized wrapper `construct.rs` adds today is
  unnecessary.
- **Bare-text leaves** (text directly in a block parent) get a shared
  `ComputedValues::initial_values_with_font_override(Font::initial_values())`
  Arc, so they contribute no padding/border/margin of their own. They're
  childless → taffy's leaf arm → sized by the parley measure fn anyway;
  display:inline (the initial) is irrelevant there.

## Arena shape

```rust
struct BoxNode<NodeId> {
    dom_id: Option<NodeId>,           // None only for anonymous/text leaves
    style: ServoArc<ComputedValues>,  // cloned from cascade (or shared initial)
    children: Vec<usize>,             // indices into the arena
    inline_content: Option<InlineContent<NodeId>>, // Some => measured leaf
    cache: taffy::Cache,
    unrounded_layout: taffy::Layout,
    final_layout: taffy::Layout,
}
struct BoxTree<NodeId> { nodes: Vec<BoxNode<NodeId>>, root: usize, node_map: FxHashMap<NodeId, usize> }
```

`taffy::NodeId` is `u64`-backed; map arena index ↔ `NodeId` via `into()`
(same as taffy's own slotmap-key round-trip).

## Traits to implement (template: taffy's own `TaffyView`)

- `TraversePartialTree` — `child_ids`/`child_count`/`get_child_id` over
  `children`.
- `TraverseTree` (marker).
- `LayoutPartialTree` — `CoreContainerStyle<'a> =
  TaffyStyloStyle<ServoArc<ComputedValues>>`, `CustomIdent = Atom`;
  `get_core_container_style` clones the node's Arc into the wrapper;
  `set_unrounded_layout`; `compute_child_layout` mirrors `TaffyView`'s
  dispatch (cache wrapper → `match (display, has_children)` →
  `compute_block/flexbox/grid_layout` or `compute_leaf_layout` with the
  parley measure fn for inline leaves / intrinsic size for `<img>`).
- `CacheTree` — `cache` get/store/clear.
- `RoundTree` — `unrounded_layout`/`final_layout`.
- `LayoutBlockContainer` / `LayoutFlexboxContainer` /
  `LayoutGridContainer` — each GAT is again
  `TaffyStyloStyle<ServoArc<ComputedValues>>`; getters clone the Arc.
- `PrintTree` — optional, skip first.

Run via `compute_root_layout(&mut tree, root, viewport)` then
`round_layout(&mut tree, root)`; read `final_layout` per node into the
`FragmentPlane`.

## Replaced-element (`<img>`) sizing

Today `apply_intrinsic_image_sizes` *mutates* the owned `taffy.size`.
With zero-copy style there's nothing to mutate. Instead `<img>` becomes
a **measured leaf**: its measure fn returns the decoded intrinsic size
(from the `ImagePlane`), clamped/overridden by any definite CSS
`width`/`height` read from its `ComputedValues`. This is how blitz does
replaced content and is cleaner than style mutation. The box-tree
construction therefore needs the `ImagePlane` (as `layout()` already
threads it via the caller).

## Increments (oracle + diff-test, then swap)

1. **Arena + traits, behind a new entry point.** Add `box_tree.rs`:
   the arena, the trait impls, and `layout_via_box_tree(dom, styles,
   images, viewport) -> (FragmentPlane, …)`. Keep the existing
   `TaffyTree`-based `layout()` as the **oracle**.
2. **Diff-test** `layout_via_box_tree` against `layout()` across the
   genet-layout lib fixtures + the `html_to_pixels_e2e` HTML corpus
   (same FragmentPlane rects within an epsilon). This is the receipt,
   mirroring the incremental-relayout diff-tests.
3. **Swap.** Point `layout()` (and `render`, `subtree`, the scripted
   relayout paths) at the box-tree; delete the `TaffyTree`-based
   `construct.rs` path and **`cv_to_taffy.rs`**; drop the owned
   `taffy: TaffyStyle` field from `StyleEntry` and
   `refresh_taffy_from_cascade` / `apply_intrinsic_image_sizes` (folded
   into measure-fn sizing).
4. **Update docs** — close the stylo_taffy plan's reframed
   done-condition; update the snapshot.

Stop-and-commit after increment 1 (arena + traits compiling + a first
diff-test green) so the work lands in reviewable batches.

## Open questions

- **OQ1 — `set_style`/mutation for incremental relayout.** The scripted
  tier's `relayout_incremental` rebuilds from the DOM today, so the
  box-tree only needs full construction first. A future
  mutate-in-place (change one node's Arc, re-layout the subtree) is a
  follow-on, not in scope here.
- **OQ2 — anonymous block boxes.** Mixed inline/block children
  currently force the whole parent to block and each text run becomes
  its own leaf; full CSS would wrap contiguous inline runs in an
  anonymous block. Out of scope; preserve today's behavior exactly
  (the diff-test enforces parity).
- **OQ3 — `detailed_layout_info`.** Keep the feature on (genet's taffy
  features include it) but no grid consumer yet; implement the no-op
  default.

## Done conditions

- `box_tree.rs` drives `layout()`; `cv_to_taffy.rs` and the
  `TaffyTree`-based `construct` are gone.
- `StyleEntry` no longer carries an owned `taffy::Style`.
- All genet-layout lib tests + `html_to_pixels_e2e` green (the
  box-tree produces pixel-identical results to the retired oracle).
- The stylo_taffy plan's reframed done-condition is closed (file
  deleted, for real this time); snapshot updated.

## Outcome (2026-05-25)

All done-conditions met. Landed in three commits:

- **Increment 1+2+2b** — `box_tree.rs`: the arena + the taffy trait impls
  (`TraversePartialTree`/`LayoutPartialTree`/`CacheTree`/`RoundTree` +
  the block/flex/grid container traits), `layout_via_box_tree`, and a
  diff-test against the `TaffyTree` oracle (10 fixtures incl. floats +
  images, ≤0.5px). Two wrinkles surfaced + fixed:
  - **Floats** needed the `BlockContext` threaded into
    `compute_block_child_layout` (the default impl drops it), *and* a
    `CssStyle` adapter forwarding `BlockItemStyle::float`/`clear` —
    `stylo_taffy 0.3.0-alpha.4`'s `TaffyStyloStyle` only overrides
    `is_table`, so floats were invisible through the zero-copy wrapper.
  - **Replaced `<img>`** sizing: the parent's block layout makes the
    stretch decision from the child's `get_block_child_style().size()`,
    so the intrinsic size has to be baked into the *child* style there
    (the same `CssStyle` carries an optional `size_override`), not just
    the leaf's own measure.
- **Increment 3** — the swap. `layout()` is a thin wrapper over the box
  tree (takes `&ImagePlane`, returns `BoxTree`); `construct.rs` reduced
  to shared gather/cascade-reader helpers; `paint_emit` emits from
  `&BoxTree`; `StyleEntry` dropped its owned `taffy: Style` (+
  `refresh_taffy_from_cascade` / `apply_intrinsic_image_sizes` /
  `taffy_style`); `render`/`render_subtree`/pelt-viewer/e2e dropped the
  refresh+apply-intrinsic pre-pass. **`cv_to_taffy.rs` deleted.**
  `build_box_tree` handles a re-rooted `SubtreeView` (an element
  `document()`) as the root directly. The lib parity tests became
  absolute-geometry assertions (oracle retired).

Receipts: `genet-layout --lib` 38, `genet-scripted --lib` 4, GPU
`html_to_pixels_e2e` 19 — all green; the full
HTML→cascade→box-tree→emit→render→readback path is pixel-correct.

**Deferred (out of scope, tracked):** mutate-in-place for incremental
relayout (OQ1 — `relayout_incremental` still rebuilds from the DOM);
anonymous block boxes (OQ2); the `size_override` rides only the block
path (no flex/grid replaced content in the corpus + no such genet
feature yet); named grid lines (the `Atom` ident now *can* flow, but
nothing consumes it). The upstream `stylo_taffy` `BlockItemStyle`
float/clear gap is a fix-candidate to offer.
