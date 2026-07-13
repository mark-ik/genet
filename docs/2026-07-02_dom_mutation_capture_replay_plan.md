# DomMutation Capture/Replay + Arena Stats Plan

**Status:** Capture/replay and genet-side arena stats closed 2026-07-02.
Mere-side apparatus surfacing remains tracked in the broader graph-delta plan.
**Date:** 2026-07-02.
**Scope:** make the `DomMutation` stream recordable and replayable (the DOM-layer
equivalent of netrender's postcard capture/replay), and give genet's arenas a
uniform stats surface. Motivated by the data-oriented doctrine brief in the mere
repo (`design_docs/2026-07-02_data_oriented_doctrine_brief.md`, §6 items 2-3):
every delta stream in the tower should be captureable at its boundary and
replayable against a fresh fold. Today `DomMutation<Id>` derives only
`Clone, Debug` (`components/shared/layout-dom/lib.rs`), so the busiest stream
in the stack cannot be recorded.

**Out of scope:** paint-emit incrementality (paint cost scaling with document
size on the RepaintOnly path). That is a separate, larger genet-layout project
and needs its own plan.

---

## Findings (seams verified 2026-07-02, corrected after first landing)

- **Bare `DomMutation` is not self-contained for offline replay.** The
  fine-grained restyle design only made it self-contained for *Stylo snapshot
  reconstruction*: `AttributeChanged` carries `old_value`, which is enough for
  restyle invalidation, not for DOM-state replay. For capture/replay the raw
  enum is missing insertion position, inserted subtree payload, new attribute
  values, new character data, and the distinction between orphaning a node
  (`remove_child`, node still live) and dropping it (`remove`, node dead). The
  recorder therefore has to enrich each drained mutation from the current DOM
  before it is replay-ready.
- **The snapshot half already exists.** `genet-scripted-dom/serialize.rs`
  serializes the DOM via html5ever (powers `outerHTML` and the verso flip's
  DOM-snapshot layer). Capture = initial serialized document + per-batch
  mutation log.
- **Current drain reality is higher than the intended final tap.**
  `IncrementalLayout::apply` is the right eventual one-place tap, but in the
  current checkout it is only exercised in `genet-layout` tests. The live
  scripted host still rebuilds a fresh `IncrementalLayout` in
  `genet-scripted::ScriptedDocument::frame` and otherwise leaves the mutation
  queue undrained. The first recorder therefore has to live at the scripted
  document's public mutation boundaries (`build`/`evaluate`/`dispatch_event`/
  `pump`) and later move down to `IncrementalLayout::apply` once a persistent
  session is live.
- **Id normalization is the one subtlety.** `NodeId` packs a per-document
  `doc_tag` into its high bits on 64-bit debug builds (the G0 fence). A capture
  must record the bare arena index (`raw & INDEX_MASK` semantics), and replay
  must remint ids against the replay document rather than calling
  `from_raw` on recorded values (the fence would assert). Arena allocation is
  deterministic (parse order, then mutation order; slots never reused), so the
  recorded index equals the replayed index; the map is identity, but the remint
  must still go through the replay document to pick up its tag.
- **Determinism caveat remains real even after the landing.** Replay reproduces
  ids only if the replay parse is the same code path as the original parse.
  Parsing the *serialized* snapshot is not byte-identical to parsing the
  original network HTML (whitespace, implied tags). The recorder therefore
  captures the snapshot *after* initial parse (serialize the live arena), and
  the layout-parity tests build the source DOM from that same serialized path.
  A stronger recorded arena-shape assertion is still possible hardening, but it
  is no longer blocking capture/replay.

## Plan

### Phase 1 — serializable mutation record

- `CapturedMutation` = `DomMutation<u64>` plus owned strings, `serde` derives,
  behind a `capture` feature on `layout-dom` (postcard as the encoding, per the
  one-wire-discipline convention). A `fn captured<Id: /* impl detail */>(m: &DomMutation<Id>, to_raw: impl Fn(&Id) -> u64) -> CapturedMutation`
  mapping helper; no derives forced onto the generic enum itself. This is the
  transportable mirror of the generic enum, not yet a replay-ready batch
  record by itself.
- Done: a drained `Vec<DomMutation<NodeId>>` round-trips through postcard bytes
  in a unit test.

### Phase 2 — the recorder

- First landing: env-gated `MutationRecorder` in `genet-scripted`, on session
  start writing the serialized initial document and then appending postcard-
  framed **replay-oriented** batches at the scripted document's public mutation
  boundaries. Each recorded mutation is enriched from the post-mutation DOM:
  inserts carry `next_sibling` + inserted `outer_html`, attribute changes carry
  `new_value`, character-data changes carry `new_data`, subtree replacement
  carries `new_inner_html`, and removals record whether the node is still live
  after the operation.
- Follow-on once a persistent incremental session is live: move the tap down to
  `IncrementalLayout::apply` and record the `Applied` outcome + viewport there,
  with an equivalent full-cascade hook for rebuild-only hosts.
- Done: browsing a page with `GENET_DOM_CAPTURE_DIR` set produces a capture
  file; nothing changes without it.

### Phase 3 — the replay harness

- `replay_capture(path) -> ReplayReport`: parse the recorded snapshot into a
  fresh `ScriptedDom`, assert arena-shape parity (Findings, last bullet), then
  drive `IncrementalLayout::apply` batch by batch, comparing each `Applied`
  classification and a fragment-plane digest against the recorded ones.
- Landed in two slices:
  - first, a headless DOM-state replayer over the replay-oriented batch
    records, rebuilding a fresh `ScriptedDom` from the snapshot and checking
    final document HTML plus live-node count;
  - then a layout-parity layer that seeds replay with the recorded stylesheet
    set and viewport size, runs a shadow `IncrementalLayout` session batch by
    batch, and compares recorded-vs-replayed `Applied`, fragment digest, and
    viewport scroll/size bits.
- Lives beside genet-layout's tests as a headless utility (also callable from
  a small `nova_cli`-style bin if useful for triage).
- Done: a failing mutation/layout batch can be reduced to a capture file that
  replays headlessly off the live host, and in-tree regression tests now cover
  DOM-state replay plus attribute-vs-structural layout parity.

### Phase 4 — arena stats

- Genet-side stats landed here; mere-side apparatus surfacing stays in the
  broader graph-delta plan
  (`repos/mere/design_docs/mere_docs/implementation_strategy/2026-07-02_graph_delta_capture_apparatus_stats_plan.md`).
- A plain stats struct per arena, one method each, no framework:
  `ScriptedDom::stats()` (nodes by kind, attribute count, estimated bytes),
  `IncrementalLayout::last_batch_stats()` (mutations in, coalesced count,
  damage class, elements restyled, boxes rebuilt), box-tree node count.
- Exposed through the existing observables seam (`engine-observables-api`) so
  hosts (apparatus panel in mere, pelt debug overlay) render real numbers.
- Landed on the genet side: `engine-observables-api` now defines shared DOM
  and layout-batch stats shapes; `ScriptedDom::stats()` reports live node-kind
  counts, attribute counts, and a rough byte estimate; and
  `IncrementalLayout::last_batch_stats()` reports apply path, coalesced
  invalidations, damage class, restyled elements, rebuilt boxes, fragment
  count, and box-tree node count when the retained side-table is valid.
- Current caveat: the live scripted host still rebuilds a fresh
  `IncrementalLayout` in `ScriptedDocument::frame`, so `last_layout_batch_stats`
  is presently "the retained session built for the last frame", not a long-lived
  mutation log. The metric becomes more interesting once a persistent
  incremental session is live.

## Progress

- 2026-07-02: plan written; seams verified (existing HTML serializer,
  current-vs-intended drain sites, id-fence normalization requirement).
- 2026-07-02: **Phase 1 landed.** `layout-dom-api` now carries a `capture`
  feature with `CapturedMutation` / `CapturedQualName`, and
  `genet-scripted-dom` exposes `capture_node_id` / `remint_node_id` so capture
  strips the debug doc-tag fence and replay re-tags ids against the replay
  document. Added a real postcard round-trip test over drained
  `DomMutation<NodeId>` batches.
- 2026-07-02: **Phase 2 partial landing.** `genet-scripted` now supports
  env-gated capture via `GENET_DOM_CAPTURE_DIR`: it writes the initial DOM
  snapshot and postcard-framed mutation batches at `ScriptedDocument`'s public
  mutation boundaries (`build`, `evaluate`, `dispatch_event`, `pump`).
- 2026-07-02: **Phase 2 recorder schema corrected.** The first generic
  `CapturedMutation` mirror turned out not to be replay-ready on its own, so
  the recorder now writes enriched batch records with insertion position,
  inserted subtree serialization, new values/text, subtree replacement HTML,
  and remove-vs-orphan liveness. The lower `IncrementalLayout::apply` tap,
  `Applied`/viewport parity, replay harness, and arena stats are still open.
- 2026-07-02: **Phase 3 partial landing.** `genet-scripted-dom` now exposes
  snapshot/subtree import helpers, and `genet-scripted` now has a headless
  `replay_capture(path)` path that rebuilds DOM state from a capture file and
  checks final document HTML plus live-node count in a regression test. The
  `IncrementalLayout::apply` parity layer, `Applied` capture, viewport parity,
  and fragment-plane digesting are still open.
- 2026-07-02: **Phase 3 completed.** Capture files now also record the
  stylesheet set plus a configurable layout viewport seed, and each mutation
  batch records `IncrementalLayout::apply` outcome, fragment-plane digest, and
  viewport bits. `replay_capture(path)` now replays those batches through a
  fresh shadow `IncrementalLayout` session and fails on any DOM/layout parity
  mismatch. Added regression tests for replayable mutation batches, orphan
  liveness, and attribute-vs-structural layout parity.
- 2026-07-02: **Phase 4 genet-side landing.** `engine-observables-api` now
  carries shared DOM/layout batch stats structs; `genet-scripted-dom` reports
  live arena counts and rough bytes; `genet-layout` reports retained batch
  counters for repaint-only, splice, and full-fallback paths; and
  `genet-scripted` exposes both through the scripted host surface. Added
  focused tests for DOM stats, retained layout stats, and structural-vs-attribute
  batch accounting.
- 2026-07-02: **Doc closed.** Capture/replay and the genet arena-stats half
  are landed; mere-side apparatus surfacing remains tracked with the broader
  graph-delta/apparatus work.
