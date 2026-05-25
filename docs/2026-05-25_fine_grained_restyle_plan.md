# Fine-grained Stylo restyle — execution plan

Status: **DONE (2026-05-25)** — the deliberately-stubbed Stylo
invalidation pipeline is un-stubbed and live; incremental restyle drives
the layout loop, skipping layout for paint-only changes. Diff-tested
against full re-cascade. See [Outcome](#outcome-2026-05-25). Executes the
6-step arc sketched in
[2026-05-20_serval_script_engine_plan.md](./2026-05-20_serval_script_engine_plan.md)
(§ "Fine-grained restyle"). Grounded in the Stylo source
(`servo/stylo` rev `572ecba`) + the current serval stubs.

## The problem

`run_cascade` re-cascades the **whole document** every pass. Stylo's
incremental machinery is deliberately stubbed in serval:

- `adapter_stylo.rs`: `has_dirty_descendants`→`false`,
  `set/unset_dirty_descendants`→no-op, `has_snapshot`→`false`,
  `handled_snapshot`→`true`, `set_handled_snapshot`→no-op,
  `compute_layout_damage`→`Default`.
- `cascade.rs`: an empty `SnapshotMap::new()`; a full preorder traversal
  from the root each call.

Fine-grained restyle un-stubs this: capture an element's **old** state at
mutation time, compare old-vs-new against the `Stylist`'s selector
dependency map, mark only the actually-affected elements, and re-cascade
just those.

## Architectural decision — snapshot capture (resolved 2026-05-25)

Stylo invalidation is **snapshot-based**: it needs an element's
pre-mutation state (classes / id / attrs / pseudo-state), captured *when
the mutation happens* (the old value is gone by restyle time). But
[`DomMutation`](../components/shared/layout-dom/lib.rs) is deliberately
**render-state-free** — the DOM provider records *what* changed and
nothing more; all style / dirty-bit / invalidation state lives on the
serval-layout side (planes design; an earlier `mark_dirty`-on-the-DOM
draft was rejected for leaking render state into the DOM).

**Decision: enrich `DomMutation` with the old value** (option 1). An old
attribute value is plain DOM data — not a dirty bit, not style, not
layout coupling — so carrying it on the mutation record keeps the
provider render-state-free *in the sense the principle means*, while all
snapshot-building + invalidation stays in serval-layout. (Option 3 —
scripted-dom owning a `SnapshotMap` — was ruled out: it pulls Stylo into
the engine-neutral mutable-DOM crate. Option 2 — a neutral side-channel
in scripted-dom — keeps it Stylo-free but puts snapshot *machinery* in
the DOM crate, more than the principle wants.)

## Verified Stylo API surface

- `style::servo::selector_parser::SnapshotMap` — `FxHashMap<OpaqueNode,
  ServoElementSnapshot>`; `get<T: TElement>(&el)` keys by
  `el.as_node().opaque()`.
- `ServoElementSnapshot` — concrete, **already** `impl ElementSnapshot`.
  Fields: `state: Option<ElementState>`, `attrs: Option<Vec<(AttrIdentifier,
  AttrValue)>>`, `changed_attrs: Vec<LocalName>`, `class_changed`,
  `id_changed`, `other_attributes_changed`. `id_attr()` / class reads come
  from `attrs`, so a correct snapshot must populate `attrs` with at least
  the old class/id entry for the invalidator to see the *old* value.
- `AttrIdentifier` (`style::servo::attr`), `AttrValue =
  <SelectorImpl as ::selectors::SelectorImpl>::AttrValue`.
- `StateAndAttrInvalidationProcessor::new(&SharedStyleContext, element,
  &mut ElementData, &mut SelectorCaches)`.
- `TreeStyleInvalidator::new(element, prev_sibling, next_sibling,
  &SharedStyleContext, processor).invalidate() -> InvalidationResult`.
- `compute_layout_damage` is serval's `TElement` method; servo/blitz
  compute `RestyleDamage` (REPAINT vs RECONSTRUCT/RELAYOUT) from old-vs-new
  `ComputedValues`.

## Increments (each diff-tested against the full-re-cascade oracle)

**1 — Snapshot data + capture + builder (this increment).**
- `DomMutation::AttributeChanged` gains `old_value: Option<String>`
  (`None` = the attr was newly added). `serval-scripted-dom::set_attribute`
  records the prior value before overwriting.
- `serval-layout`: `snapshot::build_snapshot_map(dom, &[DomMutation]) ->
  SnapshotMap` — one `ServoElementSnapshot` per changed element:
  `class_changed` / `id_changed` / `other_attributes_changed` +
  `changed_attrs`, and `attrs` populated with the **old** values (so
  `ElementSnapshot` reports the pre-mutation class/id). Coalesces multiple
  changes to one element; the snapshot holds the *original* old state.
- Test: set `class` on an element, drain, build the map, assert the
  snapshot reports `class_changed` + the old class via `ElementSnapshot`.
  Risk this de-risks: constructing `ServoElementSnapshot` / `AttrValue`.

**2 — Un-stub dirty + snapshot bits on the adapter.** `StyleEntry` gains
`dirty_descendants: Cell<bool>` + `handled_snapshot: Cell<bool>`; un-stub
`set/unset/has_dirty_descendants`, `has_snapshot` (query the active
`SnapshotMap`), `handled_snapshot`/`set_handled_snapshot`. The
`SnapshotMap` is threaded to the adapter the same way `(dom, plane)` is —
through the cascade TLS context (`CascadeCtx`).

**3 — Invalidator → restyle traversal.** A restyle entry point that, given
the prior `StylePlane` + a `SnapshotMap`, runs
`StateAndAttrInvalidationProcessor` + `TreeStyleInvalidator` per
snapshotted element (setting dirty bits via the un-stubbed methods), then
drives a traversal that re-cascades only dirty elements. Diff-test: same
computed styles as a full re-cascade for a class-toggle mutation.

**4 — RestyleDamage + wire-in.** Un-stub `compute_layout_damage`
(REPAINT-only changes skip layout); replace `relayout_incremental`'s
`RestyleSubtree` whole-subtree re-cascade with the invalidation-driven
minimal restyle. The coarse-oracle diff-test is already in place.

## Known fiddly bits / deferred

- **Full old attr set.** Snapshots populate `attrs` with the *changed*
  attrs' old values; `[attr]`-selector deps want the *complete* old attr
  vector. The class/id increment doesn't need it; full-old-attrs is
  reconstructable (old_value + current attrs) when attribute selectors
  land. Until then `attr_matches` is also still stubbed to `false`
  (a separate gap).
- **Pseudo-class state** (`:hover`, `:focus`) — needs a `StateChanged`
  mutation + `state` on the snapshot. Deferred (no interaction state yet);
  `match_non_ts_pseudo_class` is stubbed `false`.
- `SelectorCaches` / `MatchingContext` lifetimes and `ElementData` mutation
  *during* invalidation are the intricate part of increment 3 — budget for
  it; do not rush past the diff-test.

## Done conditions

- A class-toggle mutation restyles only the affected elements (verified by
  instrumentation) and produces computed styles **identical** to a full
  re-cascade (diff-test).
- `relayout_incremental` drives the minimal restyle; the coarse oracle
  diff-test stays green.
- REPAINT-only changes skip relayout (`compute_layout_damage`).

## Outcome (2026-05-25)

Done — the four increments landed across six commits (plan + 1/2/3/4a/4b):

- **1 — snapshots.** `DomMutation::AttributeChanged` carries `old_value`
  (captured in `serval-scripted-dom::set_attribute`);
  `serval_layout::build_snapshot_map` reconstructs each changed element's
  pre-mutation attr set into a Stylo `SnapshotMap`. Tests: snapshot
  reports the *old* class/id (not the live value).
- **2 — adapter bits.** `StyleEntry` gained dirty-descendant +
  handled-snapshot `Cell`s; the `TElement` `{set,unset,has}_dirty_descendants`
  / `has_snapshot` / `handled_snapshot` impls read/write them; the cascade
  TLS context carries an optional `SnapshotMap`. Full cascade unchanged.
- **3 — invalidator.** `restyle_with_snapshots`: build snapshots → mark
  the dirty path from each changed element to the root → re-run the
  cascade with the snapshots in context. Stylo's
  `ElementData::invalidate_style_if_needed` (per element, during the
  traversal) runs the real `StateAndAttrInvalidationProcessor` +
  `TreeStyleInvalidator`, setting `RestyleHint`s so only the affected
  elements recompute. `run_cascade` + `restyle_with_snapshots` share a
  `cascade_traverse` helper. Diff-tested vs full re-cascade: self-restyle
  (class a→b) **and** descendant propagation (`.box p`).
- **4a — damage signal.** `restyle_with_snapshots` returns
  `RestyleOutcome { needs_relayout }`, read from the `RestyleDamage` Stylo
  stores on `ElementData` during the restyle (`StylePlane::reset_damage` /
  `aggregate_damage`). `compute_layout_damage` stays empty — it's the
  servo-flow augmentation hook; serval lays out with taffy, so the generic
  damage is the right signal. Test: color → repaint-only, width → relayout.
- **4b — live wiring.** `IncrementalLayout` persists `StylePlane` +
  `FragmentPlane`; `apply(mutations)` routes attribute-only batches through
  the incremental restyle and **skips layout** when paint-only, structural
  batches through a full recompute. Returns `Applied` (Unchanged /
  RepaintOnly / Restyled / FullRecompute). Diff-tested vs full
  cascade+layout (color = RepaintOnly box-unchanged; width = Restyled
  matches full; append = FullRecompute).

Receipts: `serval-layout --lib` 47, `serval-scripted` 4 — green.

**Deviations from the original done-conditions, recorded:**

- The live entry point is the new `IncrementalLayout` (a stateful
  cascade+layout session), **not** the existing stateless
  `relayout_incremental` (which is FragmentPlane-splice and holds no
  `StylePlane`). Incremental restyle fundamentally needs a persistent
  `StylePlane`, so a stateful holder is the right shape; folding the
  structural FragmentPlane-splice path into `IncrementalLayout` (so one
  engine handles both restyle and structural relayout) is the natural
  follow-on, plus having `serval-scripted` adopt it as its live entry.
- `compute_layout_damage` was **not** un-stubbed (left empty) — see 4a;
  the repaint-vs-relayout signal comes from the generic `RestyleDamage`
  Stylo already computes, which is the correct source for taffy layout.

**Deferred (tracked):** full old-attr set + `attr_matches` for `[attr]`
selectors; pseudo-class state (`:hover`/`:focus`) snapshots +
`match_non_ts_pseudo_class`; merging the structural-relayout splice into
`IncrementalLayout` + serval-scripted adoption.
