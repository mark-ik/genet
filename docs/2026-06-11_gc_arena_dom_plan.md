# gc-arena DOM Plan (the piccolo fork's dividends)

**Date**: 2026-06-11
**Status**: Planned. No code written yet.
**Scope**: Put the piccolo fork (`Code/crates/piccolo`, v0.3.3, MIT) to work on
two fronts: (a) the gc-arena direction for `serval-scripted-dom` — kill the
never-reuse slab's unbounded growth and give the scripted lane real
detached-node collection; (b) a piccolo `ScriptEngine` backend as the
mod-scripting Lua option, which is also the fork's first in-tree consumer and
the conformance forcing-function for the engine-neutral seam.
**Related**: mere's actor constellation plan (Progress 2026-06-10, the
Rust+JS scripting decision this composes with — Lua here is a *pluggable
option*, not a third first-party substrate); mere's
`2026-05-24_external_deps_topology_brief.md` (the vendored-fork registry,
updated with the piccolo entry).

---

## Grounding (verified 2026-06-11)

- **The fork**: piccolo 0.3.3, gc-arena consumed as a **git dep pinned to
  kyren's rev `5a7534b`** (piccolo `Cargo.toml:22`), not a deliberate
  workspace pin yet. Nova and Boa are also vendored forks in `Code/crates/`,
  so engine-side hooks (weak refs / finalization) are patchable, not
  upstream-gated.
- **The slab** (`serval-scripted-dom/lib.rs:70-75`): `Vec<Option<Node>>`,
  slots never reused, so the arena grows monotonically with every node ever
  created. Two removal flavors exist: `LayoutDomMut::remove` drops the
  subtree (slot becomes a permanent `None`), and `remove_child`
  (lib.rs:93-104) **orphans** — keeps the subtree alive and re-insertable
  because script may hold a reference. Orphans whose JS references die are
  never freed. That is the leak this plan exists to close.
- **The hard constraint** (`lib.rs:27-33`): `NodeId(usize)` must stay
  pointer-sized — serval-layout's Stylo style-sharing cache asserts
  `size_of::<NodeId>() == size_of::<usize>()` and packs it into an
  `OpaqueElement`. On wasm32 that is 32 bits total, which rules out
  always-on generation/doc-tag packing in the id. Any reclamation design
  must keep ids monotonic (never aliased) or move liveness out of the id.
- **Who benefits, honestly**: chrome documents run no JS (xilem-serval is
  native), have zero reflectors, and remove via the dropping flavor — they
  leak only empty slots. The real beneficiary is the **scripted content
  lane** (JS holding reflectors over a long-lived document, SPA-shaped).
  Sequence accordingly: the fence and the seam work pay off now; the full
  refit pays off as the scripted tier matures.
- xilem-serval's handler registries have per-node removal
  (`context.rs:122,147,181`); whether the splice calls them on every node
  drop is a G2 audit item.

## Design rules (hold these through every phase)

1. **gc-arena never appears in a public API.** `ScriptedDom` keeps its
   `Rc<RefCell<ScriptedDom>>` host shape and its `NodeId`-based `LayoutDom`
   surface; the `Arena`/`Mutation`/`'gc` machinery is contained inside the
   crate, entered per method. Consumers (pelt-live, xilem-serval, meerkat)
   must not change for G3 beyond the documented dangle contract.
2. **Ids are never reused.** Reclamation frees node *memory*, not id
   *space*: a monotonic `NodeId` maps to a node through a weak side-table;
   a dead id resolves to "gone," never to a different node. This sidesteps
   the wasm32 width problem entirely (no generation bits needed).
3. **One gc-arena rev across the tree.** When serval takes the dep, pin it
   deliberately at the workspace level (check crates.io's latest release
   against kyren's pinned rev first, per the workspace-pins doctrine) and
   point the piccolo fork at the same pin.

## Phases (done-conditions, not dates)

### G0 — The document fence (now; independent of everything else)

Multi-root safety: a `NodeId` minted by one document used against another is
a silent wrong-node bug, and live documents are multiplying (chrome,
workbench, roster, panes, cards, now windows). Give each `ScriptedDom` a
process-unique `doc_tag`; on 64-bit debug builds, pack the tag into the
id's high bits (round-trips opaquely through Stylo's `OpaqueElement` — it is
never dereferenced) and `debug_assert` ownership in every method taking a
`NodeId`. wasm32 keeps untagged ids (the assert compiles out; native debug
runs catch the bug class). **Done when** a cross-document `NodeId` use
panics in a native debug test, and release/wasm builds are byte-identical in
behavior.

### G1 — Reflector liveness through the seam

The prerequisite for collecting script-visible nodes: the host must learn
when JS drops a reflector.

- Probe both vendored engines for death reporting: Boa's
  `WeakRef`/`FinalizationRegistry` machinery; Nova's heap (weak support
  unknown — fork-patchable if absent).
- Seam API sketch (engine-neutral, like the promise bridge):
  `ScriptEngine::drain_dead_reflectors(&mut self) -> Vec<ReflectorData>`,
  drained at the same cadence as `pump_microtasks`; backed by each engine's
  weak/finalization hooks over the existing canonical-reflector cache.
- The host layer keeps a **reflector-pin table** (ids currently held by live
  reflectors) per scripted document; a drained death unpins.
- **Fallback mode** (if an engine cannot report deaths): document-epoch
  pinning — reflector-held ids stay pinned until document teardown. This is
  today's behavior, named; navigation-bounded documents lose nothing.

**Done when** a test drops the last JS reference to a removed node, runs the
engine's GC, and the host observes the unpin on both backends (or the
fallback is the documented mode for that backend).

### G2 — The NodeId retention audit (the dangle contract)

Enumerate every place a `NodeId` outlives a frame: xilem-serval's three
handler registries (removal exists; verify the splice calls it on every
drop path), `IncrementalLayout`'s planes and side-tables, pelt-live query
results, meerkat's caches and popup anchors, undrained `DomMutation` logs
carrying ids of dropped subtrees. Define and document the contract:

> An id for an **attached** node is always live. An id for a node that was
> dropped (or orphaned and unpinned) may be dead; `dom.is_live(id)` is the
> check, and dead-id reads return the same "not found" shape as today's
> `None` slots.

Fix any site that violates it (expected: registry cleanup gaps at most —
today's `None` slots already exercise the not-found paths). **Done when**
the contract is written into `layout-dom-api`'s docs and a churn test
(create/remove/re-query across frames) passes against the slab
implementation, so G3 changes the allocator, not the contract.

### G3 — The gc-arena refit of ScriptedDom

- Internal `Arena<DomRoot>`: nodes become `Gc<'gc, NodeData>` with
  parent/children as `Gc` links; the public `NodeId` resolves through a
  monotonic-id → `GcWeak` side-table (rule 2).
- Roots: the document roots, the reflector-pin table (G1), and an explicit
  host-pin API for the rare host-held detached subtree.
- `remove_child` keeps orphan semantics, but an orphan with no pins is now
  *collectable*; `LayoutDomMut::remove` stays eager-drop in contract (its
  subtree simply becomes garbage immediately).
- Collection is incremental, paced at the `drain_mutations` boundary (the
  batching point the eager-apply design already established), with a debt
  budget so a frame never pays a full-heap pause.
- The mutation log, `LayoutDom` traits, and every consumer signature are
  unchanged (rule 1); behavior change is exactly the G2 contract.

**Done when** the churn test shows bounded memory across sustained
create/remove cycles (the slab version's monotonic growth, plotted against
the refit's plateau), pelt-live's byte-determinism suite and meerkat's
44+63 stay green, and a soak (the orrery's 400-frame sustained-motion
pattern plus DOM churn) shows no collection-pause regression in the A4-style
frame timings.

### G4 — Piccolo as a seam backend (parallel track; first fork consumer)

A `script-engine-piccolo` crate implementing `ScriptEngine`/`CallCx`:
`eval` compiles+runs a chunk; host fns via a registered module reading
`HostData`; reflectors as external userdata wrapping `ReflectorData` with a
canonical-cache table; `pump_microtasks` polls suspended executors (the
stackless VM's natural drive loop); the promise bridge as an awaitable
userdata future settled by token. Exercises the fork, makes the seam's
conformance suite real (the cross-backend tests stop being JS-only), and
delivers the modding-Lua option from the scripting discussion. Explicitly
an *option module*, not a third first-party substrate (the Rust+JS decision
stands). **Done when** the existing cross-backend seam tests (eval,
host-fn, reflector identity, host-promise settle/pump) pass on piccolo
alongside Nova and Boa, with documented deviations (e.g. no
null/undefined distinction).

### Ordering and the sooner-than-later cut

G0 is an afternoon and lands now. G1's probe and G4 can start immediately
and independently (G4 needs no DOM work at all). G2 is reading plus small
fixes and gates G3. G3 is the one structural change; it waits only on G1+G2,
not on the scripted lane maturing — but its *payoff* scales with that lane,
so if effort needs rationing, G0 → G4 → G1 → G2 → G3 is the order that
front-loads visible wins.

## Risks

- **Nova weak-hook absence** — mitigated by the fork (patch it) or the G1
  fallback mode (today's lifetime, named).
- **Arena-entry overhead** (every `ScriptedDom` method enters
  `arena.mutate`) — measure in G3's soak; expected noise-level against the
  cascade, but it is the refit's main perf unknown.
- **gc-arena API infection** — rule 1 is the guard; if `'gc` ever wants to
  escape into a public signature, stop and redesign.
- **Pin skew** — piccolo pins kyren's git rev while serval would want a
  released gc-arena; resolve at G3 entry with one workspace pin for both
  (rule 3).

## Progress

- **2026-06-11** — Plan created. Grounded against the fork
  (`Code/crates/piccolo`, 0.3.3, gc-arena `5a7534b` via git), the slab and
  its two removal flavors, the Stylo pointer-size constraint on `NodeId`,
  and the registry-removal surface in xilem-serval. No code yet; G0 is the
  entry point.
