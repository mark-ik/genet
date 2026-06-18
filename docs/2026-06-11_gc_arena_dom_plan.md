# gc-arena DOM Plan (the piccolo fork's dividends)

**Date**: 2026-06-11
**Status**: G0–G2 + G4 done (see Progress). G3 design locked 2026-06-12 to a
**custom mark-sweep**, not gc-arena (the title is now historical — kept for
stable references). Why the pivot: gc-arena's `Arena` confines every GC-data
borrow to its `mutate` closure (`for<'gc>` HRTB), but `LayoutDom`'s hot reads
return *borrows* (`element_name -> Option<&QualName>`, `attribute`, `text`,
`attributes`, `dom_children`). Backing those from `Gc<NodeData>` would force a
clone per cascade access *and* change the trait signatures — breaking rule 1.
The G3 goals (bounded memory, monotonic non-reused ids, collect
detached-unpinned subtrees) don't need gc-arena: a mark-sweep over an owned,
prunable, monotonic-keyed node store hits all of them while keeping the borrow
API byte-for-byte. gc-arena stays where it earns its keep — *inside* the
piccolo backend (G3-of-DOM owes it nothing).
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

1. **No collector machinery in a public API.** `ScriptedDom` keeps its
   `Rc<RefCell<ScriptedDom>>` host shape and its `NodeId`-based `LayoutDom`
   surface, borrows and all; the mark-sweep is a private method
   (`collect`) the host drives. Consumers (pelt-live, xilem-serval, meerkat)
   must not change for G3 beyond the documented dangle contract. (This rule
   is *why* gc-arena lost — see the Status note; its `'gc` brand can't be
   contained behind a borrow-returning read.)
2. **Ids are never reused.** Reclamation frees node *memory*, not id
   *space*: a monotonic `NodeId` keys an owned, **prunable** node store; a
   pruned (dead) id resolves to "gone," never to a different node. This sidesteps
   the wasm32 width *packing* problem (no generation bits needed). It does
   not sidestep id-space *exhaustion*: `usize` is 32 bits on wasm32, and
   reclamation-without-reuse never reclaims id space, so monotonic minting
   is itself an unbounded vector there (the one growth axis the refit
   cannot close). Realistically hours away at heavy churn, but the soak
   target in G3 is sustained create/remove, so the ceiling is named, not
   assumed away. If it ever bites, the fix is id recycling behind the same
   side-table (which forces generation bits back in, and the wasm32
   packing problem with them) — a deliberate later trade, out of scope
   here.
3. **One gc-arena rev across the tree.** When serval takes the dep, pin it
   deliberately at the workspace level (check crates.io's latest release
   against kyren's pinned rev first, per the workspace-pins doctrine) and
   point the piccolo fork at the same pin.

## Phases (done-conditions, not dates)

### G0 — The document fence (DONE 2026-06-11)

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

### G1 — Reflector liveness through the seam (REAL on Boa + seam/host done 2026-06-11; Nova fallback)

The prerequisite for collecting script-visible nodes: the host must learn
when JS drops a reflector.

**Probe verdict + resolution (2026-06-11):** real death-reporting needs the
canonical reflector cache to hold reflectors *weakly* and sweep collected
entries. Per-backend:

- **Boa — DONE (real).** `boa_gc` exposes the weak primitive
  (`WeakGc::upgrade`), but the `JsObject → WeakGc` bridge was private
  (`JsObject::inner: Gc<…>`). Added an additive vendored patch to
  `Code/crates/boa` (`serval` branch, same HEAD as the build): a public
  `JsObject::downgrade() -> WeakJsObject` + `WeakJsObject::upgrade()`.
  serval's `[patch.crates-io]` now points boa_engine/boa_gc at the local
  path (like the vendored piccolo fork) so the patch builds without a
  push. `script-engine-boa`'s cache now holds `WeakJsObject`;
  `drain_dead_reflectors` sweeps and reports collected reflectors. Tested
  end-to-end: canonical identity holds while referenced, then drop +
  `force_collect` is reported and swept.
- **Nova — DONE (real).** The WeakRef route turned out tractable: a
  reflector `EmbedderObject` *is* already a valid `WeakKey` (the enum lists
  it), so no enum surgery was needed. Three vendored changes to
  `Code/crates/nova` (serval-embedder branch, same HEAD as the build):
  1. *Additive native API* on `EmbedderObject`: `into_weak_ref` /
     `from_weak_ref` (over the existing pub(crate) `WeakRef::set_target` /
     `get_target` + `Heap::create`) and a `clear_weak_ref_kept_objects`
     wrapper for the spec's `ClearKeptObjects`.
  2. *Completed an incomplete weak path*: the fork's
     `WeakRefHeapData::sweep_values` never nulled a collected target (it
     index-shifted as if it survived); now it sweeps the target as a weak
     reference (`sweep_weak_reference` → `None` when collected). Without this
     `Deref` could never observe a death.
  3. *Fixed a pre-existing `Global` root leak*: `Global` has no `Drop`, so
     the native-callback trampoline leaked a permanent `heap.globals` root
     for every argument and return value — pinning any reflector created or
     passed through a callback. The trampoline now `take`s those handles.

  `script-engine-nova`'s canonical cache now holds a `WeakRef` (rooted via
  `Global`, target weak) per reflector; `drain_dead_reflectors` derefs each
  and reports the collected ones. Tested end-to-end
  (`reflector_for_reports_death_after_gc`). serval's nova_vm patch repointed
  to the local path (same bare-clone trade-off as boa).
- **Piccolo — DONE (real, no fork).** piccolo *is* gc-arena
  (`UserData::into_inner` → `Gc::downgrade` → `GcWeak`). The canonical cache
  is now an in-arena `ReflectorCache` singleton holding
  `GcWeak<UserDataInner>`; `drain_dead_reflectors` sweeps and reports
  collected reflectors. Tested (`reflector_for_reports_death_after_gc`):
  canonical `==` holds while referenced, then drop + `lua.gc_collect()` is
  reported and swept. No fork patch needed.

The bare-clone trade-off (local-path boa patch) and the option to push the
patch upstream to mark-ik/boa to restore it are noted in serval's root
`Cargo.toml`.

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
fallback is the documented mode for that backend). **Status:** the seam
method, the host `ReflectorPins` table, and the `pump_and_retire` drain are
landed. **The "observes the unpin" branch is now real on all three backends**
(Boa, Nova, and piccolo each pass `reflector_for_reports_death_after_gc`:
drop the last reference, collect, and `drain_dead_reflectors` reports the
id). Done-condition fully met.

### G2 — The NodeId retention audit (the dangle contract) (DONE 2026-06-11)

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

**Landed (2026-06-11).** `LayoutDom::is_live(id) -> bool` added to
`layout-dom-api` with the contract as its doc (default `true` for immutable
backends; `ScriptedDom` overrides it — live iff the id is this document's and
its slab slot is still `Some`, covering both attached and orphaned-but-kept,
dead once dropped, false for a foreign id, never panics). Churn test
`dangle_contract_churn_across_frames` + `is_live_is_false_for_dropped_and_foreign_ids`
green against the slab.

**Audit verdict** — as predicted, "registry cleanup gaps at most," no
correctness violations, because ids never alias (a dead id matches nothing
live) and every cross-frame holder reads through an `Option`-returning lookup:

- *xilem-serval handler registries* (`click_handlers` / `key_handlers` /
  `pointer_handlers`, `HashMap<NodeId, _>`): hold ids across frames but read
  via `.get()` (Option) and are cleared by `unregister_*` on view teardown.
  A missed unregister is a bounded **memory leak**, not a dangle — a live
  hit-test never asks for a dead id, and a dead id never resolves to a
  different node. (G3 makes the orphan it might pin collectable, so the pin
  table, not the registry, is the liveness root.)
- *`IncrementalLayout` / `StylePlane` side-tables* (keyed by `D::NodeId`):
  rebuilt/spliced each layout; the cascade only ever reads attached nodes
  (always live). Stale entries are memory, swept on splice.
- *pelt-live query results* (`collect_elements_by_name` → `Vec<NodeId>`, the
  a11y tree, `range_rects`): all **per-frame transient**, attached-only — no
  cross-frame retention.
- *undrained `DomMutation` logs*: a `Removed { node, .. }` carries a
  detached id, but consumers *apply* it (remove), never read live data off
  it; the log is drained each frame.
- *meerkat caches / popup anchors*: meerkat is **cross-repo** (the mere
  workspace), not in serval. It consumes this contract; its 44+63 suites are
  the consumer-side guard and are audited there, not here.

No serval-side fixes were required; the contract is documented and the slab
churn test is the regression guard, so G3 changes the allocator, not the
contract.

Note for G3: the "attached node is always live" half of the contract
holds only if *every* document root is a gc-arena root — including the
secondary roots `create_document` mints (`lib.rs:108-110`), which live in
the same arena. A missed root collects an attached subtree, which breaks
the contract rather than merely dangling, so root registration is a G3
checklist item, not an implementation detail.

### G3 — The mark-sweep refit of ScriptedDom (DONE 2026-06-12; gc-arena superseded — see the Status note)

**The store.** `Vec<Option<Node>>` → an owned, prunable map keyed by a
*monotonic* `NodeId` (`HashMap<usize, Node>` with a **deterministic** hasher
— a tiny no-dep FNV `BuildHasherDefault` — so iteration order never varies by
seed or target, keeping pelt-live byte-determinism airtight on wasm too). A
`next_id` counter mints ids; pruning removes map entries, so memory is bounded
by *live* nodes, not ever-created. One structure: parent/children stay
`NodeId` links inside `Node`, so the store *is* the graph (no parallel
side-table to keep in sync — that was the gc-arena design's tax).

**The collector** is a private `collect(extra_roots)` mark-sweep:

- *Mark* by **undirected** reachability (parent *and* child edges) from
  {the document roots} ∪ {`extra_roots`}. Undirected matters: a pin on a deep
  orphan node must keep its whole connected component — JS can walk
  `parentNode` up and children down and re-insert any of it.
- `extra_roots` are the host's live-reflector ids (G1 `ReflectorPins`), passed
  in. The DOM stays **pin-agnostic** — it takes "extra roots," not reflector
  knowledge, so the layering holds.
- *Sweep* prunes every unmarked entry. Cycles (parent↔child) are no problem
  for a mark. `LayoutDomMut::remove` keeps its eager drop; `remove_child`
  keeps orphan semantics, but an unpinned orphan is now reaped by the next
  `collect`.
- **Roots** = the primary document root (permanent). `create_document`
  secondaries are added to the roots list too (refinement #4: a secondary
  whose attached subtree must stay live needs to be a mark root; auto-collect
  of a *dropped* secondary is a later refinement — treat it as pin-kept
  then). Fragments (`create_fragment`) are pin-kept orphans, not roots.

**Cadence (this is where C's promptness lives, as timing not bookkeeping).**
The host calls `collect(pins)` at three moments: the `drain_mutations`
boundary, an idle/timer tick (so a quiet backgrounded document still reaps
its dead orphans), and right after an unpin (prompt reclamation of a
just-orphaned-and-now-unreferenced subtree). No per-subtree pin counting —
the mark recomputes liveness each call.

**Unchanged (rule 1):** the mutation log, the `LayoutDom`/`LayoutDomMut`
traits, every consumer signature, and the borrow-returning reads. The only
behavior change is exactly the G2 contract (`is_live` can now go false for an
orphaned-then-unpinned id).

**Done when** the churn test shows the store **plateaus** across sustained
create/remove/collect cycles (vs the slab's monotonic growth); a
quiet-document variant confirms an idle `collect` reaps orphans with no
further mutations; an unpinned orphan is collected while a pinned one
survives (and its whole component with it); pelt-live's byte-determinism
suite and meerkat's 44+63 stay green; and a soak (the orrery's 400-frame
pattern plus DOM churn) shows no `collect`-pause regression in the A4-style
frame timings.

**Landed (2026-06-12).** `serval-scripted-dom`'s store is now
`HashMap<usize, Node>` over a no-dep deterministic FNV `BuildHasherDefault`,
keyed by a monotonic `next_id`; the G0 fence's pack/index feed it unchanged.
`collect(extra_roots)` is the undirected mark-sweep; `live_node_count` is the
bounded-memory signal. The host helpers `collect_dom(dom, &pins)` and
`pump_retire_collect(engine, pins, dom)` (in `serval-scripted`) drive it from
the G1 pin set. Tests: `collect_reaps_unpinned_orphans_keeps_pinned_components`
(undirected — a pin on a deep leaf spares its ancestors),
`collect_bounds_memory_under_churn` (store back to baseline after 500×9-node
cycles while `next_id` climbs past 4000 — the plateau vs the slab's growth),
`idle_collect_reaps_orphans_without_mutations`,
`secondary_document_subtree_survives_collect`, and
`collect_dom_uses_pins_as_roots`. All 14 scripted-dom + 7 serval-scripted
tests green; downstream stays green (serval-layout 55, xilem-serval 48,
pelt-live 21 **incl. `cascade_is_deterministic`**, script-runtime-api 159);
**serval-scripted-dom compiles for wasm32**.

Two carve-outs remain. (1) *Cadence wiring* — **correction (2026-06-12)**: the
persistent host loop *does* exist — it's `script-runtime-api::Runtime` (owns the
engine + the `ScriptedDom` in `HostState`, runs `run_microtasks` /
`run_event_loop` / `run_timers`). The pin table was moved down to
`serval-scripted-dom` as `Pins` (keyed on `NodeId`) so `Runtime` can use it
through its existing dep — *not* via a dep on the Nova-specific, layout-dragging
`serval-scripted`. **The tick is now wired (2026-06-12).** Audited the reflector surface: **19**
`reflector_for` handoffs in `dom.rs`, **zero** `make_reflector` — all 19 now
route through `reflect_pinned` (pin the node in `HostState.pins`, then
`reflector_for`), so pin-on-mint is complete (no unpinned handoff that
`collect` could UAF). `Runtime::collect_garbage` retires the engine's reported
deaths → `dom.collect(pins)`. Tested via `gc_tick_on_boa` (a detached node is
pinned-on-mint and spared, then reaped once unpinned). **Deliberately not
auto-fired** inside `run_microtasks` yet — it's an explicit embedder call for
now; flipping it to the microtask-checkpoint cadence is a one-line change, left
until the live webview driver exercises it (so the conformance path isn't
silently collecting mid-campaign). **That live driver is pelt V4** — the
scripted profile (`pelt --engine scripted`) in
`docs/archive/2026-06-12_pelt_development_plan.md` (archived), which the pelt plan already names as
"the gc-arena DOM plan's first real scripted workload." It's the only consumer
that calls `collect_garbage` at a frame cadence, and it's a few phases out (V0
done, V1 next), so the explicit-call shape is the right resting state until
then. (2) *Soak*: the orrery 400-frame A4-timing soak is a runtime perf check
not run here; `collect` is O(live nodes) and called at cadence, so it's
algorithmically cheap, but the empirical no-regression confirmation **is pelt
V4's "soak page that churns nodes under script"** — the same workload that flips
carve-out #1's cadence. So both remaining carve-outs land together, in pelt V4;
the collector itself is mechanism-complete.

*Resolved 2026-06-12*: the former secondary-doc carve-out is done — a
`create_document` secondary is now **pin-kept**, not a permanent root, so a
dropped `createHTMLDocument` collects like any other orphan (no leak). This
also collapsed the roots list to just the primary root (`collect` seeds from
`self.root` + the pins). Safe because the host pins a secondary's reflector
while script holds it; test `secondary_document_is_pin_kept`.

### G4 — Piccolo as a seam backend (CLEAN SURFACE DONE 2026-06-11; promise bridge deferred)

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
null/undefined distinction). **Status:** the clean surface (eval, value,
globals, native host-fns, native-data reflectors + canonical cache) is
landed and passes the conformance shapes for those; the host-promise
bridge is the deferred piece (Mark's call 2026-06-11: clean surface now,
promise later). Note the synergy for G1: piccolo *is* gc-arena, so the
weak-cache death-reporting path needs no fork patch here — it is the one
backend where the real G1 reclamation can land in-tree, making piccolo a
natural proving ground for both halves of this plan.

### G4-promise — the host-promise bridge for piccolo (DONE 2026-06-11)

The deferred half of G4. The seam's host-promise bridge
(`new_host_promise` → script `await`s it → `settle_host_promise` resumes →
`pump` drives the continuation) maps to JS Promise + microtask queue. Lua
has no Promise; the natural analog is **coroutines** (yield the running
thread until the host settles). Piccolo's primitives exist: a `Callback`
can return `CallbackReturn::Yield`, and `Executor::resume` feeds a value
back into a parked yield.

**Shape:**

- **The awaitable.** `new_host_promise` mints a `PendingPromise` userdata
  carrying the `PromiseToken` and returns it. Script suspends on it via a
  `p:await()` method (metatable on the userdata — reads better in Lua than a
  free `await(p)`, and namespaces cleanly). `await` is a callback that reads
  the token and returns `CallbackReturn::Yield`, parking the current
  executor.
- **HostSlot gains** `waiters: HashMap<PromiseToken, StashedExecutor>`
  (coroutines parked on a token) and `settled: HashMap<PromiseToken,
  StashedValue>` (values settled *before* anyone awaited — the
  settle-before-await race; `await` checks `settled` first and resumes
  immediately).
- **`settle_host_promise(token, outcome)`** looks up the parked executor and
  `resume`s it with the settled value (resolve), or resumes it *raising* a
  Lua error at the await point (reject → catchable by `pcall`). If no waiter
  yet, stash into `settled`.
- **`pump` gets real semantics** (today it is always `Quiescent`): drive the
  runnable (resumed) executors to their next yield/completion, honoring
  `Budget::Steps` (one resume = one step), returning `Pending` while
  runnable executors remain — finally distinguishing piccolo's pump from the
  synchronous-eval path.

**Deviations to document:** `await` is an explicit method, not syntax; no
Promise combinators (`.then`, `Promise.all`) unless built on top; `eval` of
a chunk that awaits *yields* rather than returning its final value (the host
sees completion via `pump` after `settle`, like top-level await); rejection
surfaces as a Lua error at the await point.

**Effort + risk:** ~1 module, ~150–250 LOC in `script-engine-piccolo`, plus
adapting the `host_promise_bridges_js_await` conformance shape to the
coroutine-await form. Risk concentrates in the yield/resume threading across
the executor boundary, the settle-before-await race, and budget accounting
on resume. **Done when** the adapted host-promise conformance shape
(settle-resume, reject-catch, settle-before-await, double-settle no-op)
passes on piccolo, with the deviations above documented.

**Landed (2026-06-11).** Built on piccolo's *executor-level* yield/resume
rather than Lua `coroutine.create` (cleaner): the global `await(p)` callback
grabs its own executor via `Execution::executor`, stashes it in
`HostSlot::waiters` keyed by token, and returns `CallbackReturn::Yield` — so
a top-level `await` suspends the `eval` executor itself (`eval` returns; the
chunk's later statements wait). `settle_host_promise` `resume`s the parked
executor with the value or `resume_err`s it with a raised Lua error, then
`pump` (`lua.finish`) drives it to completion. `settled`/`waiters`/`runnable`
tables in `HostSlot` handle the settle-before-await race. The promise is a
`UserData::new_static(PromiseTokenData(token))` (a distinct type from a
reflector's `u64`, so `downcast_static` tells them apart). Test
`host_promise_bridges_lua_await` covers all four shapes; notably piccolo's
`pcall` *is* yield-across-able, so the reject path catches cleanly. The
`p:await()` *method* form and `Budget::Steps` honoring are the only deferred
niceties (deviations documented at the crate).

### Ordering and the sooner-than-later cut

G0 is an afternoon and lands now. G1's probe and G4 can start immediately
and independently (G4 needs no DOM work at all). G2 is reading plus small
fixes and gates G3. G3 is the one structural change; it waits only on G1+G2,
not on the scripted lane maturing — but its *payoff* scales with that lane,
so if effort needs rationing, G0 → G4 → G1 → G2 → G3 is the order that
front-loads visible wins.

## Risks

- **Nova weak-hook absence** — Boa's weak hook is now landed (vendored
  `JsObject::downgrade` patch); Nova's is the remaining lift (weak-global or
  `EmbedderObject` finalization on the serval-embedder branch). Mitigated
  meanwhile by the G1 fallback mode (today's lifetime, named).
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
- **2026-06-11** — Folded four review refinements into the phase
  done-conditions: (1) the id side-table must be a prunable swept map or
  the leak relocates into it; (2) monotonic ids are an unbounded id-space
  vector on wasm32, named in rule 2 rather than assumed away; (3)
  collection needs an idle/timer tick so quiet documents still collect
  orphans, not only a drain-boundary tick; (4) every document root
  (including `create_document`'s secondaries) must be a gc-arena root or
  G2's "attached is always live" half breaks. No phase reordering.
- **2026-06-11** — **G0 landed** in `serval-scripted-dom/lib.rs`. Each
  `ScriptedDom` mints a process-unique `doc_tag` from a global atomic; on
  64-bit debug builds the tag packs into a `NodeId`'s high 16 bits
  (index in the low 48) and a centralized `index()` accessor
  `debug_assert`s ownership on every slab read, with `opaque_id` and the
  `raw()`/`from_raw()` reflector round-trip carrying the packed value
  through unchanged. On release and wasm32 the `fence` module and the
  field cfg out entirely, so ids are the bare index as before. Tests:
  `cross_document_node_id_panics` (fenced-only, `should_panic`),
  `secondary_root_is_same_document` (no false positive across
  `create_document`), `distinct_documents_get_distinct_tags`; all 8
  scripted-dom tests green, release build warning-clean, and
  serval-layout / serval-scripted / script-runtime-api still build
  unchanged (rule 1 held). Next entry point: G4 (piccolo backend, no DOM
  dependency) or G1's reflector-death probe.
- **2026-06-11** — **G1 seam + fallback landed.** Added
  `ScriptEngine::drain_dead_reflectors(&mut self) -> Vec<ReflectorData>` to
  `script-engine-api` (default = empty = the epoch-pin fallback, documented).
  Added `ReflectorPins` (per-document pin/unpin/retire/clear table, keyed on
  `ReflectorData`, engine-agnostic) and `pump_and_retire` (pump + drain →
  retire) at the `serval-scripted` crate root. Probe verdict recorded above:
  real death-reporting is a fork patch on both backends (Boa
  `JsObject::downgrade`; Nova weak-global / `EmbedderObject` finalization), so
  both ship the explicit fallback override with the precise patch named at the
  impl site. Tests: `pin_unpin_and_retire`, `clear_is_the_epoch_pin_teardown`,
  and `nova_fallback_keeps_pins_until_teardown` (mints a reflector, drops it,
  pumps, asserts the pin survives — the fallback as the documented mode); all
  green across script-engine-api / script-engine-boa / serval-scripted, rule 1
  consumers unchanged. **Open decision for Mark:** do the two fork hooks now
  (real-GC reclamation, but cross-repo churn on mark-ik/boa + mark-ik/nova),
  or stay on the documented fallback until the scripted lane matures (G3's
  payoff window). Next: G4 (piccolo backend, independent of this decision).
- **2026-06-11** — **G4 clean surface landed.** New crate
  `components/script-engine-piccolo` (added to the workspace), implementing
  `ScriptEngine` + `ScriptEngineLive` + `CallCx` over the vendored piccolo
  fork: `eval` (load → `Executor` → first result), value→string, globals,
  native host-fns (trampoline `Callback` capturing an `Rc<HostSlot>` — the
  piccolo analogue of Nova's `[[HostDefined]]`), and native-data reflectors
  (`UserData::new_static` carrying the `NodeId`, with a `StashedUserData`
  canonical cache for `node == node`). Six conformance tests green:
  reflector round-trip, value surface (`'a'..(1+2)` → `"a3"`), global
  reflector reachability, native-fn + host-data + reflector-arg,
  `reflector_for` canonical identity, and pump/deviation. Documented
  deviations: null/undefined both → Lua `nil`; no Promise (host-promise
  methods error / no-op; `pump` is always `Quiescent`). Whole script-engine
  family + serval-scripted still build. Pin note: piccolo pulls gc-arena
  `5a7534b` transitively; serval takes no direct gc-arena dep until G3
  (rule 3's workspace pin is resolved there). Next structural piece: G2
  (the dangle-contract audit), which gates G3.
- **2026-06-11** — **G1 real reclamation landed on Boa** (Mark green-lit the
  fork patch). Vendored an additive patch to `Code/crates/boa` (`serval`
  branch): public `JsObject::downgrade() -> WeakJsObject` +
  `WeakJsObject::upgrade()` (plus a manual `Debug` to keep the fork
  warning-clean). Repointed serval's `[patch.crates-io]` boa_engine/boa_gc
  to the local path (consistent with the vendored piccolo fork; bare-clone
  trade-off + the push-upstream alternative documented in root
  `Cargo.toml`). `script-engine-boa`'s canonical cache now holds
  `WeakJsObject`, and `drain_dead_reflectors` sweeps + reports collected
  reflectors. New test `reflector_for_reports_death_after_gc`: canonical
  `===` identity holds while referenced, then drop + `boa_gc::force_collect`
  → drain reports `[0x42]`, second drain empty. All 7 boa + 6 piccolo + 6
  serval-scripted tests green. Nova stays on the documented fallback (bigger
  fork lift). Also **scoped the piccolo host-promise bridge** (see the
  G4-promise section): coroutine-yield awaitable, `settle`→`resume`, real
  `pump`, with deviations. Remaining reclamation lift: Nova weak hook,
  piccolo in-tree weak cache. Next structural piece: G2 → G3.
- **2026-06-11** — **G1 real reclamation landed on piccolo (no fork).** The
  canonical cache moved from a Rust-side strong `StashedUserData` map to an
  in-arena `ReflectorCache` singleton holding `GcWeak<UserDataInner>`
  (piccolo has no `__mode` weak tables and doesn't re-export gc_arena, so
  added a direct gc-arena dep pinned to piccolo's exact rev `5a7534b` — no
  skew, local to this backend, rule-3 pin still resolved at G3).
  `reflector_for` upgrades-or-mints; `drain_dead_reflectors` sweeps the weak
  map. Test `reflector_for_reports_death_after_gc` (7 piccolo tests green).
  **Nova weak hook investigated, paused for decision:** a reflector is an
  `EmbedderObject`, which is neither markable as a weak-global without GC
  mark/sweep surgery nor a valid `WeakKey` (so the existing `WeakRef`
  machinery can't target it without adding an `EmbedderObject` `WeakKey`
  variant across the weak internals). Both are deep multi-site fork changes
  — see the G1 Nova bullet. Recommend the WeakKey-variant route as a focused
  pass; Nova stays on the documented fallback meanwhile. Real reclamation now
  on **2 of 3** backends (Boa first-party JS + piccolo). Next: promise
  bridge, then G2.
- **2026-06-11** — **G1 real reclamation landed on Nova — all three backends
  now real.** The feared deep enum surgery wasn't needed (`EmbedderObject` is
  already a `WeakKey`). Vendored three changes to `Code/crates/nova`: (1) an
  additive `EmbedderObject::into_weak_ref`/`from_weak_ref` +
  `clear_weak_ref_kept_objects` native API; (2) completed the fork's
  incomplete weak-nulling — `WeakRefHeapData::sweep_values` now nulls a
  collected target via `sweep_weak_reference` (it previously index-shifted a
  dead target, so `Deref` could never report a death); (3) fixed a
  pre-existing leak — `Global` has no `Drop`, so the native-callback
  trampoline leaked a permanent `heap.globals` root per arg/return value,
  pinning every reflector; it now `take`s them. `script-engine-nova`'s
  canonical cache holds a `WeakRef` per reflector; `drain_dead_reflectors`
  derefs and reports the dead. Repointed nova_vm to the local vendored path.
  Test `reflector_for_reports_death_after_gc` green; serval-scripted's
  `non_canonical_reflector_pin_survives_until_teardown` re-framed (Nova is no
  longer fallback). All script-engine + serval-scripted + script-runtime-api
  tests green. Next: the promise bridge (G4-promise), then G2.
- **2026-06-11** — **G4-promise landed** (piccolo host-promise bridge). Global
  `await(p)` suspends the running executor (via `Execution::executor`, stash,
  and `CallbackReturn::Yield`); `settle_host_promise` resumes it
  (`resume`/`resume_err`); `pump` drives the runnable set to completion;
  `HostSlot` gained `waiters`/`settled`/`runnable`. Executor-level yield (not
  Lua `coroutine.create`) so a top-level `await` suspends `eval` itself.
  `host_promise_bridges_lua_await` passes all four shapes (resolve, reject via
  `pcall`, settle-before-await, double-settle no-op); 8 piccolo tests green.
  Deviations documented (global `await(p)` not `p:await()`; `pump` drains
  fully). Next: **G2** (the dangle-contract audit), which gates G3.
- **2026-06-11** — **G2 landed** (dangle-contract audit). Added
  `LayoutDom::is_live(id)` to `layout-dom-api` with the contract as its doc
  (default `true`; `ScriptedDom` overrides via a non-asserting `try_index` +
  slab-slot check — live for attached and orphaned-but-kept, dead once
  dropped, false for a foreign id, never panics). Churn tests
  `dangle_contract_churn_across_frames` and
  `is_live_is_false_for_dropped_and_foreign_ids` green against the slab.
  Audited every cross-frame `NodeId` holder (xilem-serval registries,
  `IncrementalLayout`/`StylePlane`, pelt-live queries, `DomMutation` logs):
  no correctness violations — ids never alias and every holder reads through
  an `Option` lookup, so the worst case is a bounded registry memory leak
  cleaned on view teardown, exactly the plan's prediction. meerkat is
  cross-repo (mere), audited there. No serval-side fixes needed. **G3 is now
  unblocked** (G1 + G2 done); its done-conditions already carry the four
  folded refinements (prunable side-table, wasm32 id ceiling, idle collection
  tick, secondary-root registration).
- **2026-06-12** — **G3 design pivot + landed.** Surfaced that gc-arena's
  `Arena` confines GC-data borrows to its `mutate` closure, which can't serve
  `LayoutDom`'s borrow-returning hot reads without cloning + a rule-1 trait
  change. Explored the real design space (the borrow API forces an owned,
  borrowable store regardless of reclaimer; the only variable is the
  reclamation trigger) and locked **A — a hand-rolled undirected mark-sweep**
  over a prunable monotonic-keyed store, with C's promptness folded in as
  *cadence* (drain + idle + post-unpin) rather than per-subtree bookkeeping.
  Implemented: store → `HashMap<usize, Node>` (deterministic FNV hasher, so
  byte-determinism holds incl. wasm), monotonic `next_id`, document-roots
  list, `collect(extra_roots)` mark-sweep, `live_node_count`; host drivers
  `collect_dom` / `pump_retire_collect` in `serval-scripted`. 5 new collection
  tests (bounded-memory plateau, undirected pin-component, idle reap,
  secondary-root safety, host pin-roots) + all prior green; downstream green
  (serval-layout / xilem-serval / pelt-live incl. determinism /
  script-runtime-api); compiles for wasm32. Then **did carve-out #3**:
  secondary documents are pin-kept, not permanent roots, so a dropped
  `createHTMLDocument` auto-collects (no leak) and the roots list collapses to
  the primary root. Remaining carve-outs (need other pieces, not serval-layout):
  live-loop cadence wiring (waits on the persistent scripted host loop) and the
  orrery perf soak (runtime harness). **All of G0–G4 are now done.**
- **2026-06-12** — **Pin table relocated (G3 cadence prep).** Moved the
  reflector-pin table from `serval-scripted` down to `serval-scripted-dom` as
  `Pins`, keyed on `NodeId` (was `ReflectorData`/`u64`) so it lives next to the
  `collect` it feeds and carries no engine-type coupling; `serval-scripted`
  re-exports it as `ReflectorPins` and keeps the engine-coupled helpers
  (`pump_and_retire`/`collect_dom`). Chosen over making the *generic*
  `script-runtime-api` depend on the Nova-specific, `serval-layout`-dragging
  `serval-scripted` (which would invert the layering and pull `nova_vm` into the
  generic host). 16 scripted-dom + 5 scripted tests green; no other consumers.
  Identified the live host loop as `script-runtime-api::Runtime`; the GC-tick
  wiring (pin-on-mint + `collect_garbage`) is the next step (see G3 carve-out
  #1), pending a pin-on-mint completeness audit.
- **2026-06-12** — **GC tick wired into `Runtime`.** Audited the reflector
  surface (19 `reflector_for` handoffs in `dom.rs`, no `make_reflector`); routed
  all 19 through a new `reflect_pinned` helper so every node handed to script is
  pinned in `HostState.pins` (pin-on-mint, complete — no path can hand out an
  unpinned reflector `collect` would UAF). Added `Runtime::collect_garbage`
  (retire reported deaths → `dom.collect(pins)`), tested by `gc_tick_on_boa`.
  Left non-auto-firing (explicit embedder call) until the live webview driver
  exercises it; that driver (Runtime fed by a real rendered scripted page in
  pelt) is the last piece genuinely ahead. 56 script-runtime-api lib tests
  green.
