# Genet workspace audit — state snapshot (2026-05-24)

Point-in-time snapshot succeeding [2026-05-16_workspace_audit_snapshot.md](./2026-05-16_workspace_audit_snapshot.md)
(8 days stale). Captures what landed since — the **scripting stack**, the
**incremental-layout core**, the **paint-list extraction**, and the **Nova fork**
dependency — and reviews every dated plan doc's status against the actual tree
(verified against code, not doc-to-doc).

## Live workspace shape

Members = the 2026-05-16 set **plus four scripting crates** (`script-engine-api`,
`script-engine-nova`, `genet-scripted-dom`, `genet-scripted`); `genet-layout`,
`layout-dom-api`, `genet-static-dom` gained scripting-facing surface.

- **Scripting tier (new, native):**
  - `script-engine-api` — engine-neutral `ScriptEngine`/`ScriptEngineLive` traits
    (`make_reflector`/`reflector_data`); no engine dep.
  - `script-engine-nova` — **primary** backend, native-only (`cfg(not(wasm32))`),
    over the patched `nova_vm`. Green (reflector round-trip survives GC).
  - `script-engine-boa` — **NOT a workspace member**; standalone (own `[workspace]`).
    Boa 0.21.1 pins `icu_normalizer ~2.0.0` vs genet's parley 0.9 `^2.1.1` —
    irreconcilable, so boa cannot enter genet's graph. Quarantined as the
    conformance oracle until boa bumps icu. Green standalone.
  - `genet-scripted-dom` — mutable `NodeId` arena; `LayoutDom` + `LayoutDomMut`;
    records `DomMutation`s; `set_inner_html` via the static parser. `NodeId` is
    `usize`-backed (Stylo style-sharing cache requires pointer-sized).
  - `genet-scripted` — the reflector bridge (JS mutates the real DOM through
    `NodeId` reflectors) + the coarse/incremental relayout entry points.
- **`genet-layout` additions:** `render` (viewport wrapper over any `LayoutDom`),
  `invalidate` (`classify` + `coalesce`), `subtree` (`SubtreeView` + `render_subtree`).
  `layout-dom-api` gained `LayoutDomMut` + `DomMutation` (resolving its OQ#1).
- **Nova fork dependency:** `nova_vm` is redirected by a root `[patch.crates-io]` to
  the local clone at **`../../crates/nova/nova_vm`** (moved here from `repos/nova` on
  2026-05-24 alongside the other linebender forks; patch path updated, workspace
  resolves — verified via `cargo tree`). Clone is on branch `genet-embedder` (the
  `EmbedderObject` native-data patch, `fbca54b`), pushed to `github.com/mark-ik/nova`.
  Upstream PR to trynova/nova pending.
- **Paint-list extraction (landed):** `paint_list_api` + the `PaintCmd→Scene`
  translator moved to the netrender workspace (`paint_list_api`/`paint_list_render =
  { path = "../netrender/…" }`); genet is now a consumer. `components/shared/paint-list-api`
  and `components/paint/translator.rs` deleted (commit `15e9a0c`).
- **Build status:** workspace resolves (nova patch → `crates/nova`); scripting + layout
  crates build + test green (committed). Default `pelt`/fullweb build unaffected — the
  scripting crates are leaves; static/interactive tiers pull no `script-engine-*`.

## The live scripting loop (coarse), all diff-tested

- **JS → DOM:** `genet-scripted` wires a Nova builtin to mutate `genet-scripted-dom`
  through a `NodeId` reflector (`EmbedderObject` native data).
- **DOM → layout:** `relayout_if_dirty` (coarse full re-render on any `DomMutation`) is
  the **oracle**; `relayout_incremental` (classify → coalesce → scoped `render_subtree`
  → splice at the root's real position, coarse fallback on size change) is the
  incremental path. Both diff-tested against each other.
- Findings (in [2026-05-20_genet_script_engine_plan.md](./2026-05-20_genet_script_engine_plan.md)):
  Nova is native-only (64-bit-bound, no wasm32); wasm = the no-JS profile; Nova `Global`
  rooting confirmed; the boa/parley icu wall.

## Plans' states (verified against the tree)

| Doc | Stated status | Reality (2026-05-24) | Action |
| --- | --- | --- | --- |
| `2026-05-20_genet_script_engine_plan` | "proposed; **no implementation yet**" | **Built** — 4 crates + nova patch + incremental core, committed + tested | header fixed this pass |
| `2026-05-20_paintlist_extraction_plan` | "planned; **no code moved yet**" | **Done** — committed (`15e9a0c`) | header fixed this pass |
| `2026-05-16_layout_dom_api_design` | adopted; OQ#1 (mutation) open | **OQ#1 resolved** (`LayoutDomMut`+`DomMutation` shipped) | bump on next touch |
| `2026-05-16_genet_layout_lift_plan` | path-C lift plan | **Done** — `genet-layout` is the live engine | historical |
| `2026-05-12_genet_profile_ladder_plan` | strategy canonical; scripted tier = open interior | scripted tier now has **real crates** (core) | strategy canonical; impl partly realized |
| `2026-05-17_genet_layout_planes_architecture` | proposed (canonical layout arch) | current; invalidation **core built**, fine-grained Stylo restyle still stubbed | current |
| `2026-05-17_hekate_lanes_observables` | proposed (cross-engine) | current; Genet scripted lane partly real | current |
| `2026-05-17_paintlist_polyglot_renderer` | PM-3 design + receipts | superseded by the extraction (done) | superseded |
| `2026-05-20_stylo_taffy_adoption_plan` | planned; supersedes `cv_to_taffy` | **done (2026-05-25)** — `cv_to_taffy` now fully delegates to `stylo_taffy::convert`; floats land (e2e pixel test green). `cv_to_taffy.rs` kept (not deletable: taffy's `TaffyTree` isn't ident-generic, so `to_taffy_style`'s `Style<Atom>` can't be stored) | done |
| `2026-05-20_blitz_float_linebox_study` | study | reference (floats still a gap) | reference |
| `2026-05-08_c3` / `2026-05-09_c4` landed notes | landed | c4 Windows parity tail **runtime-verified 2026-05-25** (both present smokes green on real D3D12/DCOMP hardware) | resolved |
| `2026-05-16_workspace_audit_snapshot` | 2026-05-16 snapshot | **superseded by this doc** | superseded |
| `2026-05-23_sem_weave_smoke_test` | tooling note | current | current |

## Open threads

- **Fine-grained Stylo restyle** — the focused Stylo arc (un-stub snapshots +
  invalidation map + restyle traversal); precise 6-step plan in the script-engine doc.
  Highest-leverage incremental optimization; deliberately not rushed.
- **Incremental edges** — inheritance-context threading (the `SubtreeView` boundary),
  stale-fragment eviction for removed nodes, size-change propagation (coarse fallback today).
- **Boa/parley icu conflict** — boa quarantined; clears when boa bumps `icu_normalizer`.
  Moot for wasm (wasm = no-JS); matters only for the native conformance oracle.
- **Nova fork** — upstream PR pending; `usdt` + the 64-bit `Value` keep Nova native-only.
- **stylo_taffy adoption** — ✅ done (2026-05-25). Hand-written mapping
  retired (full delegation to `stylo_taffy::convert`); block-level floats
  land + e2e-tested. `cv_to_taffy.rs` survives as a thin default-ident
  assembler — *literal* deletion is blocked by taffy's non-generic
  `TaffyTree` (can't store `to_taffy_style`'s `Style<Atom>`); closing it
  needs the trait-impl-tree re-architecture (blitz-style), deferred.
- **c4 Windows parity tail** — ✅ resolved (runtime-verified 2026-05-25). Both
  `--windows-present-smoke` (master) and `--windows-present-surfaces-smoke`
  (per-`SurfaceKey` DCOMP child visual) present clean on this machine's
  D3D12/DCOMP path (AMD 780M / RTX 4060), exit 0. Only a cosmetic color
  screenshot remains.
- **Fork-path fixups (committed 2026-05-25):** root `Cargo.toml` + `probes/nova-probe`
  patch paths repointed to `../../crates/{nova,xilem}` after the linebender/nova forks
  moved under `crates/`; `sem_weave` doc updated for the repo-wide `weave` rollout.

## Pitfalls (carried + new)

- (carried) Profile/engine audit gates: static/interactive tiers must pull no
  `script-engine-*` / `mozjs`.
- (new) **Boa can't be a workspace member** until the icu pin clears — keep it standalone.
- (new) **The Nova fork is load-bearing** — `script-engine-nova` needs the
  `genet-embedder` branch (`EmbedderObject` patch). The `[patch.crates-io]` path tracks
  the clone location (`crates/nova` now); keep it in sync if the clone moves.
- (new) Scripted `NodeId` must stay `usize`-backed (Stylo style-sharing cache assertion).

## Addendum — 2026-05-25 review (post-snapshot developments + the rendering pipeline)

A state review the day after this snapshot. Verified against the tree
(`cargo check -p pelt` green 2026-05-25).

### Box tree — the "leave `TaffyTree`" move is in flight, not deferred

The Open-threads `stylo_taffy` entry above calls the trait-impl-tree
re-architecture "deferred"; as of 2026-05-25 02:52 it's **in progress**.
**`box_tree.rs`** (`2026-05-25_box_tree_trait_impl_plan`) is genet's own
box-tree arena implementing taffy's trait-impl tree (`LayoutPartialTree` +
traversal / style-access traits), the style accessor returning
`stylo_taffy::TaffyStyloStyle` **zero-copy** over `Arc<ComputedValues>` — no
per-node `Style` rebuild. **Increment 1 (arena + traits + `layout_via_box_tree`)
has landed and is wired** (`layout()` is now a thin wrapper over it); the old
`TaffyTree` path stays as the diff-test oracle until parity, then the swap
deletes the `construct` TaffyTree path + `cv_to_taffy.rs` and drops
`StyleEntry.taffy`.

This is the convergence point for three threads listed separately above —
stylo_taffy, the float gap, the parley-leaf IFC seam. Owning the tree is the
prerequisite the blitz-float study identified; the box tree now has "the same
shape as blitz-dom," so the deeper float work (text-wrap-around-floats,
anonymous block boxes — box-tree OQ2) becomes *reachable* (not delivered) once
the swap lands. The big architectural call was made and is executing.

### Absent from the snapshot: the rendering / host-integration pipeline

This doc covers engine internals thoroughly but says nothing about **how genet
reaches a screen**. That arc exists and builds clean:

- **`pelt-viewer`** — a Xilem app: nav bar + a `WebContent` Masonry widget that
  reserves an `External` layer (paints nothing itself).
- **Zero-copy compositing** — netrender is booted on Masonry's *shared* wgpu
  device via `AppDriver::on_wgpu_ready`; genet content renders to a texture on
  that device and is `copy_texture_to_texture`'d into the External layer's
  bounds — **no GPU→CPU readback**. Resize falls out (External bounds drive the
  render size); relative `<img>` / `<link>` resolve via a local-file loader.
- Depends on the **masonry_winit External-layer realization seam**
  (`crates/xilem`, commit `694cc7f`) — additive, default-no-op
  `composite_external_layers` hook.

**This pipeline has no plan doc** — the one part of genet that puts pixels on
screen has no design record. Recommend writing one.

### Load-bearing forks — offer-don't-push

genet carries **two** un-upstreamed load-bearing fork patches:

- **nova** — `crates/nova`, branch `genet-embedder`, `EmbedderObject`
  native-data patch (`fbca54b`). Gates scripting.
- **xilem** — `crates/xilem`, the masonry_winit External-layer realization
  (`694cc7f`) on `mere-wgpu-29-vello-0-9`. Gates the zero-copy viewer.

Policy: **don't proactively upstream, don't bug maintainers.** Each patch
carries a **proposed-upstream note** — drafted, ready to offer *if* a maintainer
ever shows interest — in
[`2026-05-25_fork_upstream_proposals.md`](./2026-05-25_fork_upstream_proposals.md).
The pitch is on hand; nothing is pushed. (The xilem seam is the cleaner pitch —
it *finishes* an existing upstream placeholder: `VisualLayerKind::External` was
designed-but-unrealized.) This supersedes the "Nova fork — upstream PR pending"
line above: not pending, just ready.

### Scripting reach (native-vs-wasm) — deliberate non-priority, not a contradiction

Nova is native-only (64-bit `Value`, `usdt`); "wasm = no-JS" is **by design and
fine**: browser-embedded hosts (extension, PWA) already run inside a browser
with working JS, so genet needn't provide it there — its value in those
contexts is the **"everything else"** (smolweb + p2p-protocol experiences via
netrender that Chrome/Firefox won't render anyway). A wasm-JS path exists if
ever needed (wasm64; boa's `icu_normalizer` pin clearing; Nova on nightly) —
explicitly deferred. Near-term target: **render the open web on native** —
already a wild result. One thing at a time. ("genet everywhere, maximally
featured" stays the long heart's-goal, not a near-term constraint.)

### What's next (re-prioritized)

1. **box tree → parity diff-test → swap** (Increments 2–4): delete
   `cv_to_taffy.rs` + the TaffyTree `construct` path; drop `StyleEntry.taffy`.
   In flight.
2. **Fine-grained Stylo restyle** — highest-leverage incremental-layout
   optimization (the focused-arc plan; stands).
3. **Mere integration** — the inker paint-list adoption consuming genet's
   `GenetPaintList` / External-layer bridge; turns the engine into the product.
4. *Then* the deeper float / IFC work (text-wrap-around-floats), now reachable
   on the owned tree.

Restyle (#2) vs Mere-integration (#3) is a "make it fast" vs "make it real" bet
worth naming explicitly.
