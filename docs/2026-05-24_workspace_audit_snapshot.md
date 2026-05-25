# Serval workspace audit — state snapshot (2026-05-24)

Point-in-time snapshot succeeding [2026-05-16_workspace_audit_snapshot.md](./2026-05-16_workspace_audit_snapshot.md)
(8 days stale). Captures what landed since — the **scripting stack**, the
**incremental-layout core**, the **paint-list extraction**, and the **Nova fork**
dependency — and reviews every dated plan doc's status against the actual tree
(verified against code, not doc-to-doc).

## Live workspace shape

Members = the 2026-05-16 set **plus four scripting crates** (`script-engine-api`,
`script-engine-nova`, `serval-scripted-dom`, `serval-scripted`); `serval-layout`,
`layout-dom-api`, `serval-static-dom` gained scripting-facing surface.

- **Scripting tier (new, native):**
  - `script-engine-api` — engine-neutral `ScriptEngine`/`ScriptEngineLive` traits
    (`make_reflector`/`reflector_data`); no engine dep.
  - `script-engine-nova` — **primary** backend, native-only (`cfg(not(wasm32))`),
    over the patched `nova_vm`. Green (reflector round-trip survives GC).
  - `script-engine-boa` — **NOT a workspace member**; standalone (own `[workspace]`).
    Boa 0.21.1 pins `icu_normalizer ~2.0.0` vs serval's parley 0.9 `^2.1.1` —
    irreconcilable, so boa cannot enter serval's graph. Quarantined as the
    conformance oracle until boa bumps icu. Green standalone.
  - `serval-scripted-dom` — mutable `NodeId` arena; `LayoutDom` + `LayoutDomMut`;
    records `DomMutation`s; `set_inner_html` via the static parser. `NodeId` is
    `usize`-backed (Stylo style-sharing cache requires pointer-sized).
  - `serval-scripted` — the reflector bridge (JS mutates the real DOM through
    `NodeId` reflectors) + the coarse/incremental relayout entry points.
- **`serval-layout` additions:** `render` (viewport wrapper over any `LayoutDom`),
  `invalidate` (`classify` + `coalesce`), `subtree` (`SubtreeView` + `render_subtree`).
  `layout-dom-api` gained `LayoutDomMut` + `DomMutation` (resolving its OQ#1).
- **Nova fork dependency:** `nova_vm` is redirected by a root `[patch.crates-io]` to
  the local clone at **`../../crates/nova/nova_vm`** (moved here from `repos/nova` on
  2026-05-24 alongside the other linebender forks; patch path updated, workspace
  resolves — verified via `cargo tree`). Clone is on branch `serval-embedder` (the
  `EmbedderObject` native-data patch, `fbca54b`), pushed to `github.com/mark-ik/nova`.
  Upstream PR to trynova/nova pending.
- **Paint-list extraction (landed):** `paint_list_api` + the `PaintCmd→Scene`
  translator moved to the netrender workspace (`paint_list_api`/`paint_list_render =
  { path = "../netrender/…" }`); serval is now a consumer. `components/shared/paint-list-api`
  and `components/paint/translator.rs` deleted (commit `15e9a0c`).
- **Build status:** workspace resolves (nova patch → `crates/nova`); scripting + layout
  crates build + test green (committed). Default `pelt`/fullweb build unaffected — the
  scripting crates are leaves; static/interactive tiers pull no `script-engine-*`.

## The live scripting loop (coarse), all diff-tested

- **JS → DOM:** `serval-scripted` wires a Nova builtin to mutate `serval-scripted-dom`
  through a `NodeId` reflector (`EmbedderObject` native data).
- **DOM → layout:** `relayout_if_dirty` (coarse full re-render on any `DomMutation`) is
  the **oracle**; `relayout_incremental` (classify → coalesce → scoped `render_subtree`
  → splice at the root's real position, coarse fallback on size change) is the
  incremental path. Both diff-tested against each other.
- Findings (in [2026-05-20_serval_script_engine_plan.md](./2026-05-20_serval_script_engine_plan.md)):
  Nova is native-only (64-bit-bound, no wasm32); wasm = the no-JS profile; Nova `Global`
  rooting confirmed; the boa/parley icu wall.

## Plans' states (verified against the tree)

| Doc | Stated status | Reality (2026-05-24) | Action |
| --- | --- | --- | --- |
| `2026-05-20_serval_script_engine_plan` | "proposed; **no implementation yet**" | **Built** — 4 crates + nova patch + incremental core, committed + tested | header fixed this pass |
| `2026-05-20_paintlist_extraction_plan` | "planned; **no code moved yet**" | **Done** — committed (`15e9a0c`) | header fixed this pass |
| `2026-05-16_layout_dom_api_design` | adopted; OQ#1 (mutation) open | **OQ#1 resolved** (`LayoutDomMut`+`DomMutation` shipped) | bump on next touch |
| `2026-05-16_serval_layout_lift_plan` | path-C lift plan | **Done** — `serval-layout` is the live engine | historical |
| `2026-05-12_serval_profile_ladder_plan` | strategy canonical; scripted tier = open interior | scripted tier now has **real crates** (core) | strategy canonical; impl partly realized |
| `2026-05-17_serval_layout_planes_architecture` | proposed (canonical layout arch) | current; invalidation **core built**, fine-grained Stylo restyle still stubbed | current |
| `2026-05-17_hekate_lanes_observables` | proposed (cross-engine) | current; Serval scripted lane partly real | current |
| `2026-05-17_paintlist_polyglot_renderer` | PM-3 design + receipts | superseded by the extraction (done) | superseded |
| `2026-05-20_stylo_taffy_adoption_plan` | planned; supersedes `cv_to_taffy` | **in progress** — `cv_to_taffy.rs` still present | active |
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
- **stylo_taffy adoption** — `cv_to_taffy` not yet retired.
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
  `serval-embedder` branch (`EmbedderObject` patch). The `[patch.crates-io]` path tracks
  the clone location (`crates/nova` now); keep it in sync if the clone moves.
- (new) Scripted `NodeId` must stay `usize`-backed (Stylo style-sharing cache assertion).
