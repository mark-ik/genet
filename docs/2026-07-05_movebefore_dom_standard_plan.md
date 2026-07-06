# moveBefore: move-preserving reparenting as the DOM standard contract

**Date**: 2026-07-05
**Status**: Plan. Slice 1 (mutation vocabulary + ScriptedDom semantics) lands with
this doc; later slices are staged below with WPT expectation flips as done
conditions.
**Context**: The WHATWG DOM spec grew `Node.moveBefore()` (shipped in Chromium
early 2025): an atomic move that preserves state `removeChild`/`insertBefore`
destroys — iframe documents, CSS animations/transitions, focus, popovers, custom
elements get `connectedMoveCallback` instead of disconnect/connect. That is the
splice/graft contract (`BoxTree::graft_subtree`, `incremental.rs`) as a web
standard. Mere's one-state-N-windows design
(`repos/mere/design_docs/mere_docs/design/2026-07-05_one_state_n_windows_design.md`)
needs exactly this for chrome tear-out: with one forest ScriptedDom, moving a
tile between window roots is a same-document `moveBefore`. Chrome tear-out and a
page reparenting a live iframe become the same code path.

The spec's own constraint ratifies the forest topology: `moveBefore` throws
rather than adopting across documents. Same-document is the only case, which is
the case the forest design creates.

## What exists (receipts)

- **WPT is on disk and wired.** `tests/wpt/tests/dom/nodes/moveBefore/` (the
  full upstream suite) with `ports/serval-wpt` expectations currently `"fail"`
  in `dom_boa.json` / `dom_nodes_boa.json` (crash tests `"skip"`). The done
  condition per slice is flipping specific expectations, not writing tests.
- **`ScriptedDom::insert_before` is already a move for in-tree nodes** (detach,
  then reinsert; same-parent index resolved post-detach) but it emits only
  `DomMutation::Inserted`. The detach is silent: no consumer learns the child
  left its former parent. Under the forest design this is a live defect (a
  cross-window move would never invalidate the source window's session), and it
  is also wrong for the plain page case per spec (`insertBefore` of an in-tree
  node is remove + insert, both observable).
- **`DomMutation` (components/shared/layout-dom/lib.rs) has no move variant.**
  Consumers: serval-layout (`cascade.rs`, `incremental.rs`, `invalidate.rs`,
  `snapshot.rs`), serval-scripted (`capture.rs`, `lib.rs`), xilem-serval
  (`runner.rs` tests), and meerkat (`pane_session.rs`, `render/cards.rs`,
  `window_view/gnode_pool.rs`).
- **JS bindings**: `script-runtime-api/dom/bootstrap.js` defines
  `Node.prototype.insertBefore` over native `__insertBefore` (registered in
  `dom/mod.rs`). `moveBefore` is a sibling entry point.

## Slices

### S1 — Mutation vocabulary + ScriptedDom semantics (this session)

- `DomMutation::Moved { node, from_parent, to_parent }` in layout-dom, plus the
  `capture` feature's serializable mirror.
- `ScriptedDom::move_before(parent, child, reference)`: same tree surgery
  `insert_before` already does, emitting one `Moved` record. No-op when the
  resolved position equals the current position (spec: still no state reset,
  and nothing changed to report).
- Fix `insert_before`'s silent detach: an in-tree child now emits
  `Removed { former_parent }` before `Inserted`, matching spec observability.
  A same-parent reorder through `insert_before` is deliberately still
  remove + insert (that is what the standard says `insertBefore` does; state
  preservation is `moveBefore`'s contract, not `insertBefore`'s).
- Consumers take conservative `Moved` arms: treat as removed-from +
  inserted-under (invalidate both parents, route to both windows' sessions).
  Behavior identical to a remove + insert pair; no fast path yet.

Done when: serval-scripted-dom unit tests cover move (cross-parent, same-parent
reorder, no-op position, mutation records), all consumers compile with explicit
arms, serval-layout + xilem-serval tests pass, meerkat builds and its
partition-routing tests pass.

### S2 — Layout fast path: Moved rides the splice

**Landed with S1, by construction (2026-07-05).** No dedicated `Moved` handling
was needed in `apply_structural`: it is already general over invalidation
roots, so S1's classification (a cross-parent `Moved` yields both parents as
restyle roots) walks `try_splice_at` for the source and target subtrees like
any structural batch. Verified by two tests in `incremental.rs`:
`cross_parent_move_splices_incrementally` (one atomic `Moved` record, two
coalesced scopes, `Applied::Spliced`, moved fragment oracle-matches a full
recompute, retained emit matches a fresh session command-for-command) and
`same_parent_move_reorder_splices_with_one_scope` (one scope, reorder lands,
emit parity). The usual splice preconditions gate as ever: a parent whose
outer size responds to the moved child (auto-height) falls back to the full
path, correctly.

Still open, evidence-gated: reusing the moved subtree's *existing* boxes and
shaped text at the new position instead of re-laying the subtree inside the
target's scoped pass. Today the subtree re-lays-out once within the target
splice — already scoped, already emittable. Don't build the box-reuse graft
until a profile shows that scoped re-layout hot (the ancestor-escalation
lesson in `apply_structural`'s comment: a measured loss, removed).

The `child-style-preserve.html` WPT flip moves to S3's done conditions — it
needs the script surface to run at all.

### S3 — Script surface

**Code + runtime test landed 2026-07-05.** Native `__moveBefore`
(`query_traverse.rs`, the unchecked primitive, mirroring `__insertBefore`) and
`Node.prototype.moveBefore` in `bootstrap.js` with the pre-move validity gates:
`HierarchyRequestError` for a non-container target, a non-element/character-data
node, text-into-document, a would-be cycle, and the same-root rule (a move
never adopts — both nodes under this document, or both inside the same detached
tree); `NotFoundError` for a reference that is not a child of the target.
Custom elements get the spec's fallback pair (disconnected + connected) on a
connected move; `connectedMoveCallback` itself is S4 (the registry does not
capture it yet). Covered by `dom::tests::dom_move_before_works` (cross-parent
move, reorder, in-place no-op, all three throw classes, return value,
detached-tree move) — green on boa alongside the full runtime-api suite.

**WPT receipts (2026-07-05, `serval-wpt testharness dom/nodes/moveBefore`,
boa).** Subtests went 0 → 21/112 across the suite:

- `Node-moveBefore.html` (the core semantics file): **19/32 subtests pass**.
  The file-level expectation stays `"fail"` (file status is all-or-nothing).
- `script-move-before.html`: **2/2, all-pass** — expectation flipped
  `"fail"` → `"pass"` in `dom_nodes_boa.json` + `dom_boa.json`.
- `preserve-render-blocking-script.html` improved `error` → `fail`: the test
  used to throw calling the missing `moveBefore`; now the API exists and it
  fails on the actual render-blocking assertion (an S4 subsystem).
  Expectations updated to match.
- Every other failing file names its owning S4 tranche: CSS
  transition/animation continuation, focus/focus-within, selection, shadow
  DOM + slots, custom-element move reactions, iframes, popover/dialog/
  fullscreen, MutationObserver, live ranges.
- Full `dom/nodes` (330 files) and `dom/` runs are green against their
  expectation files (`unexpected=0`), which also checks the
  `insert_before`/`append_child` observability fix caused no regression
  anywhere in the suite. Two unrelated stale entries surfaced in `dom_boa.json`
  (`legacy-pre-activation-behavior`, `xpath-result-single-node-value-nullable`,
  both now passing from concurrent engine work) and were flipped to `"pass"`.

Remaining inside S3's scope: nothing — the surface is in and receipted.
Further subtest gains in `Node-moveBefore.html` belong to the S4 tranches.

### S4 — State-preservation tranches, by subsystem

The suite's preservation tests gate on subsystems, not on moveBefore itself.
Flip each tranche as its subsystem exists:

- style/animation continuation (`continue-css-*`, `css-transition-*`) with the
  animation engine;
- focus preservation (`focus-preserve*.html`) with engine focus;
- custom element `connectedMoveCallback` with custom elements;
- iframe document preservation when content iframes exist.

Not scheduled here; each is listed so the expectation file stays an honest map
of what serval can and cannot preserve.

### S5 — xilem_serval keyed views ride it

**Same-parent half landed 2026-07-05.** A keyed reorder over single-element
children is now a real move:

- Vendored `xilem_core::ElementSplice` grew a defaulted
  `hoist_pending(n) -> bool` (false by default, so every other impl and
  backend is untouched).
- `ServalChildrenSplice` implements it: reorder the pending queue, then one
  `move_before` — the node never detaches.
- `Keyed::seq_rebuild`, gated on `Seq::ELEMENTS_COUNT == Count::One`: a
  leading run of removals tears down first (so pure removals cost zero
  moves), then a later-matched survivor hoists to the cursor instead of
  tearing down the intervening entries; they stay pending and are consumed
  in order. Non-single-element children and non-hoisting splices keep the
  old teardown + build contract, and the fallback paths tolerate the
  out-of-order consumption holes a hoist leaves.
- Receipt: `keyed_reorder_moves_the_element_without_teardown` — same
  NodeIds swapped, exactly one `Moved` record, zero builds, zero teardowns,
  both children rebuilt in place. The middle-insert / middle-delete tests
  keep their old assertions (removal costs no moves).
- Free consumer: meerkat's gloss recent rows + minimap squares are
  `Keyed<GraphMemberId, _>` over single Views, so their reorders stop
  churning DOM nodes with no meerkat change. Meerkat-side verification is
  owed once the tree compiles again (blocked 2026-07-05 by the in-flight
  `linked-data` `spareval`/`rdf-12` bump, unrelated).

**Cross-parent half landed 2026-07-06: `PortableKeyed` + the ctx nursery.**

- **Prerequisite fix, all four event wrappers** (`on_click` / `on_key` /
  `on_pointer` / `on_wheel`): a registration is a `(node, path)` pair and
  rebuild now reconciles **both** — the old code re-registered only on a node
  change, assuming the path structural. An adopted subtree keeps its node but
  changes its path; without this, a moved tile's handlers routed to the freed
  old path and went `Stale`. Teardown also unregisters by the *stored* path
  (correct under adoption and in nursery drains).
- **Splice ops**: `extract_pending` (take the element out of the source
  splice's bookkeeping, leave the DOM node attached — parking is not removal)
  and `adopt_pending` (move the still-attached foreign node into place — one
  atomic `Moved` — and queue it as next pending, so the ordinary mutate-based
  rebuild consumes it). Both defaulted no-op in vendored `xilem_core`, so
  non-hoisting splices and other backends degrade to teardown + fresh build.
  This resolves the sharp edge named above: the source's cursor accounting
  treats the extracted child as consumed while its node stays put.
- **`ServalCtx` nursery**: typed buckets per `(K, V)` instantiation, erased
  behind a drain trait; a monomorphized teardown fn pointer is captured at
  park time so the drain needs no type knowledge. The runner drains at the
  end of every rebuild (looping, since a teardown can park nested children):
  unclaimed = the key left every list = real teardown + node removal.
- **`PortableKeyed<K, V: View + Clone>`** (`portable.rs`): single-`View`
  children (the tile/card shape). Departing keys park instead of tearing
  down; arriving keys claim + adopt + rebuild from the parked view/state
  (which re-registers handlers at the new path); same-parent reorders hoist
  as in `Keyed`; every fallback (non-extracting, non-adopting, nursery miss)
  degrades to the old teardown + build contract.
- **Ordering caveat, on the record**: preservation requires the source
  sequence to rebuild before the target (view tree order); target-first
  degrades safely (fresh build + drained park — correct, no leak, no
  preservation). The multi-projection runner (one-state-N-windows step 2)
  makes this host-controllable by rebuilding the source window first.
- Receipts (`tests.rs::portable`): the done condition holds —
  `cross_parent_move_preserves_element_state_and_handlers` (same NodeId under
  the new parent, exactly one `Moved { from_parent, to_parent }`, zero
  builds/teardowns, and a dispatched click on the moved tile still routes,
  proving the path reconciliation); plus the target-first fallback and the
  park-then-drain removal tests. Suites: xilem-serval 83, serval-layout 251,
  xilem_core, meerkat check + gloss/pane/gnode tests all green.

Remaining beyond S5: nothing engine-side. The consumer work (meerkat tiles as
`PortableKeyed` children, the forest dom, source-first window rebuild order)
belongs to the one-state-N-windows sequencing, not this plan.

## Non-goals

- Cross-document adoption semantics. `moveBefore` throws there by design, and
  the forest topology makes same-document the only case Mere needs.
- Animation/focus/iframe preservation ahead of their subsystems (S4 tranches).
