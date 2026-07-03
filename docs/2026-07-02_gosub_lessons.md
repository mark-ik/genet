# Lessons from gosub-io/gosub-engine

**Date:** 2026-07-02
**Status:** research / harvest. Companion to `2026-06-24_formal_web_lessons.md`;
not itself a plan. Nothing applied yet; candidate homes at the foot.
**Source:** [github.com/gosub-io/gosub-engine](https://github.com/gosub-io/gosub-engine)
and the sibling repo [gosub-sonar](https://github.com/gosub-io/gosub-sonar).
Studied via repo reading (crate tree, `gosub_lattice` doc + `lib.rs`, sonar
README, workspace `Cargo.toml`), mapped against serval + mere seams (file:line).
**Why it matters:** gosub is not a rigor playbook the way formal-web was. It is
a parallel from-scratch engine converging on the same component decomposition
as Mere (taffy 0.10 + parley 0.11 + vello 0.8 + wgpu 29, engine-as-library
behind an event/command embedder interface), and its author is deliberately
peeling engine organs into standalone crates. Three of those organs line up
one-to-one with Mere organs and named gaps.

## What gosub is

Active (commits 2026-07-01/02; 3.7k stars; essentially one author, jaytaph).
A modular embeddable engine: spec HTML5/CSS3 parsers with html5lib-tests and
fuzzing, a zones/tabs model driven by `TabCommand` in / `EngineEvent` out,
swappable render backends (null / cairo / skia / vello), a V8 JS lane, an
examples matrix (winit/egui/gtk4 x backends) plus a headless screenshot bin.
Rendering is not pixel-perfect yet; overall fidelity is years behind serval's
Servo-derived lanes. So the harvest is components and shapes, not discipline.

## 1. `gosub_lattice`: the reference for real CSS table layout

A standalone CSS 2.1 §17 table layout engine (MIT, deps: anyhow + log only,
decoupled from the rest of gosub in commits this week). It never owns a tree:
the host implements a `TableTree` trait over its own structures; `layout_cell`
is the recursion hook where lattice hands cell-content layout back to the host
(taffy block layout, in their case); `set_layout` writes geometry back. Entry
point `compute_table_layout(&mut tree, table_node, avail_w, avail_h)` returns
the table's border-box size. Five phases: model (role classification +
anonymous boxes), grid (slot-filling), column widths, row heights (via
`layout_cell`), placement.

### What adopting it as reference buys

1. **It closes the exact deferrals the first cut names.** serval-layout's
   `build_table` flattens cells into a taffy grid and defers colspan/rowspan,
   `border-collapse`, and `<caption>` placement (`box_tree.rs:399-405`, `:486`);
   `ua_defaults.rs:81` defers the same plus fixed table-layout. Lattice
   implements colspan/rowspan slot-filling (rowspan clamped to section
   boundaries), auto and fixed table-layout column distribution, border-spacing
   placement with header -> body -> footer ordering, and anonymous table-box
   generation per §17.2.1 (serval currently drops stray content,
   `box_tree.rs:548`). Each deferral maps to an implemented lattice phase, so
   the port is guided by working code rather than derived from spec prose.
2. **Correctness class, not increment.** Today any spanning table lays out
   wrong: every cell occupies one grid slot, so spanned cells collide or leave
   holes, and column widths come from taffy grid auto-sizing rather than the
   table width-distribution algorithm. Real-web tables (infoboxes, wikis, docs
   sites) use colspan constantly; this is the difference between "tables render"
   and "tables approximately stack".
3. **The integration shape matches serval-layout exactly and keeps the taffy
   fork clean.** Host-owned tree + `layout_cell` recursion + `set_layout`
   writeback means serval-layout keeps owning the box tree, taffy keeps laying
   out cell content, and the table algorithm becomes a leaf module inside
   serval-layout instead of a taffy-fork feature (no new fork skew). The seam
   already exists: `table_inside` intercepts before the taffy hand-off
   (`box_tree.rs:404`), so the work replaces `build_table` in place.
4. **Test furniture comes with it.** Lattice ships `src/mock.rs`, a fixture
   `TableTree` for driving the algorithm from hand-built trees. That matches
   serval-layout's existing hand-computable integer-px test style
   (`box_tree.rs:1551`) and gives the table module its own unit surface with
   neither DOM nor WPT in the loop; WPT `css/css-tables` then measures
   conformance the same way `custom-elements` measured the interface table.
5. **Cheap to hold.** Algorithm-only crate, two deps, freshly standalone. It
   reads as a self-contained algorithm text. Whether consumed as a dependency
   (if it gets published) or read-and-adapted under the borrow-technique rule,
   the dependency direction stays one-way.

### Limits to plan around

Not implemented in lattice: rowspan height distribution, `border-collapse:
collapse` positioning (parsed only), `<col>`/`<colgroup>` width contributions,
`caption-side`, `vertical-align`, and fixed layout's fixed-width algorithm.
Its narrow-column distribution uses a 14px-floor heuristic that is not spec
text. So: reference implementation and starting algorithm, with WPT
`css/css-tables` as the correctness authority, not lattice itself.

## 2. gosub-sonar vs netfetcher: the missing scheduler layer

Where the two overlap, netfetcher is the deeper artifact: it owns the WHATWG
Fetch algorithm (CORS/tainting, RFC 6265bis cookies, RFC 9111 cache, HSTS,
SRI, referrer policy, content-decode, h3 and ws lanes); sonar is young
(14 commits) and has none of that depth. What sonar has is the layer netfetcher
deliberately leaves to the host (netfetcher is a directly callable library;
grep confirms zero priority/coalescing/cancellation/concurrency machinery in
its `src/`):

- priority-queued scheduling (a `Priority` per request),
- inflight coalescing: concurrent requests for one resource fan a single
  network fetch out to multiple subscribers,
- per-origin concurrency caps,
- refcounted request handles carrying cancellation tokens,
- buffered-vs-streaming with a `max_bytes` cap as API surface (meerkat
  hand-rolls this per call site, `meerkat/src/fetch.rs:205`),
- `Initiator` / `ResourceKind` classification per request (prioritization and
  attribution hang off it),
- a `metrics` feature exposing engine counters over a local HTTP endpoint
  (gosub-engine feature flag), which fits the real-observability rule.

Today meerkat's loader actor calls `netfetcher::fetch` directly per request,
which is fine for one document at a time. The lesson activates when a serval
fullweb page fans out tens of subresources: the scheduler should be one layer,
either a `Fetcher` module inside netfetcher or the host loader actor, with
sonar as the reference shape for its API (handle = subscribe + cancel +
priority). A feature checklist for that layer, not a port target.

## 3. Zones per graph

Gosub's zone is the embedder-visible isolation unit: cookies, storage, and
network state belong to the zone; tabs within a zone share them; commands and
events stay per tab. Gosub's own notes flag zone lifecycle cleanup as the
complexity concentrator.

Mere mapping: **graph = zone.** A window is a graph-shaped session, so the
isolation boundary is the graph, not the window or the webview. All web-bearing
nodes in a graph share one partition (cookie jar, HTTP cache, HSTS, preflight
cache, DOM storage); tear-out keeps zone membership because the zone travels
with the graph; rekey moves a node into the target graph's zone. Different
graphs then carry different personas/logins for the same site for free.

The seams are already shaped for this:

- netfetcher's `FetchContext` is caller-owned, and its storage seams are
  trait-backed explicitly "so the host can back it with persona- or
  session-scoped partitions" (netfetcher README; `context.rs`). Nothing in
  netfetcher changes.
- meerkat today has exactly one implicit zone: a process-global `session_jar()`
  static (`meerkat/src/fetch/cookies.rs:17`) with `session_context()` building
  per-fetch contexts around it and permissive defaults for the other stores
  (`cookies.rs:250`). Zone-per-graph replaces that static with a per-graph map
  keyed by graph identity, owned by the loader actor.
- the serval-side DOM-storage seam already exists: the `StorageProvider` host
  hook (`script-runtime-api/platform.rs:119`, serval `3ed0ed0`, tested on boa
  and nova), built by thread 6b of the native session store plan
  (`mere/design_docs/mere_docs/implementation_strategy/2026-06-23_native_session_store_plan.md`).
  Its remaining host impl keys the partition `(persona, origin)`; zone-per-graph
  is an amendment to that key, not a new seam. (The Servo lane has its own
  storage thread keyed `WebViewId` + origin, `components/shared/storage/
  webstorage_thread.rs`; only relevant if serval-scripted becomes a live
  meerkat lane.)

## Noted, not carried

- `gosub_webinterop` / `gosub_webexecutor`: proc-macro JS-binding codegen over
  a JS-engine abstraction; the opposite bet from the hand-written interface
  table serval already shipped (`archive/2026-06-24_html_interface_table_plan.md`),
  and one real backend (V8) in practice. Counterpoint only.
- parser fuzzing (`gosub_html5`/`gosub_css3` fuzz dirs): transfers to nematic's
  smolweb parsers only; serval's html5ever/stylo lanes are upstream-tested.
- null render backend + `bin/gosub-screenshot` packaging: Mere already has the
  scry-shots harness; only the in-repo packaging is novel.

## What does NOT transfer

- The HTML5/CSS3 parsers: html5ever + stylo are mature and spec-tested;
  gosub's exist to be independent, not better.
- The V8 lane: the browser doctrine bars a nested JIT; Boa/Nova is the path.
- `gosub_taffy`: serval's taffy fork is deeper than their integration.
- `RenderBackend` trait swapping: netrender genericization already shipped, and
  scry/graft/weld covers engine multiplexing at a higher seam.
- The zones/tabs embedder API itself (`TabCommand`/`EngineEvent`): serval and
  meerkat already have their own actor/command seams; the zone concept
  transfers, the API shape does not.

## Where to apply (candidate homes)

- **Tables:** a scoped serval-layout plan of its own when picked up; the
  first-cut deferrals live at `box_tree.rs:399-405`/`:486` and
  `ua_defaults.rs:81`, and this doc is the lattice reference pointer.
- **Fetch scheduler:** a netfetcher or meerkat-loader plan when a fullweb
  multi-subresource lane lands; sonar's feature list above is the checklist.
- **Zones-per-graph:** the partition keys are owned by the native session store
  plan (phase 4 keys cookies `cookies/<persona>` over eidetic; thread 6b keys
  DOM storage `(persona, origin)`); zone-per-graph amends those keys with graph
  identity. Graph-identity semantics (tear-out keeps membership, rekey moves
  zones) come from mere's multi-graph / multi-window planning.
