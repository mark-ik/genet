# Pelt Development Plan — serval's reference shell

**Date**: 2026-06-12
**Status**: In progress. **V0 done** — the present core moved into serval as
`components/serval-winit-host`. **Render-driver reform done** — the host
render-driver extracted into `components/serval-render` and `pelt-live` retired
(the V0-shaped move that cleared V1's foundation; see Progress). **V1** (the static
viewer) is next.
**Role statement (the decision this plan rests on):** pelt is **serval's
servoshell** — the minimal reference browser that proves the engine
standalone, drives engine development without mere's graph machinery, and is
what an outside contributor clones and runs. meerkat remains the product
shell; pelt deliberately stays thin so it keeps demonstrating that browsers
are cheap to assemble on serval. The role just became reachable: the xilem
fork is a git dep (bare clones build), Masonry left the tree with
pelt-viewer's retirement (2026-06-12, workspace audit snapshot), and the
smoke suite already makes pelt the validation entrypoint.
**Related**: the pelt-viewer retirement note
(`2026-05-16_workspace_audit_snapshot.md`, 2026-06-12 update); mere's host
cheap-path plan (C1's laid-out-document query object is pelt's eventual query
seam too); the gc-arena DOM plan (V4 is its first real scripted workload);
the pseudo-element follow-ups (every done-condition there wants V3's reftest
harness).

---

## Grounding (current state, verified this week)

- `pelt` bin = smoke launcher + a retired-viewer exit message. `--engine
  <browser|viewer|static|headless>` parses into `pelt_core::EngineProfile`;
  capabilities print; browser is rejected; nothing renders.
- `pelt-core` = the shell contracts (EngineProfile, ShellEngine,
  ResourceFetcher). `ResourceFetcher` has been definition-only since the
  retirement dropped its sole consumer.
- `pelt-desktop` = desktop host contracts + the platform present smokes
  (windows-dxgi / macos-calayer / wayland-subsurface / netrender / webgl) +
  a smoke-shaped `static_viewer` scaffold.
- `components/serval-render` = the serval host render-driver (the lib formerly
  inside the `pelt-live` probe): ScriptedDom / LayoutDom → `netrender::Scene`
  (`scene_from_layout_dom` / `scene_from_scripted_dom`), the host spatial queries
  (`hit_test_node`, fragments, caret), and `accesskit_tree`, with the
  cascade-determinism + host-spine tests. `pelt-live` (the winit counter demo) was
  **retired** with the extraction (2026-06-12); pelt's own viewer subsumes it.
- The shared present plumbing (`RenderCore` + `WindowSurface`) now lives in serval
  as `components/serval-winit-host` (V0, 2026-06-12) — the backwards-pointing piece
  is gone. With `serval-render` these are the two serval host cores: scene
  *production* and *presentation*.
- serval has **no reftest harness** (serval-wpt covers JS-harness tests
  only), and nothing drives full-page `<script>` end to end
  (script-runtime-api + Nova/Boa exist; no full-document consumer).

## Non-goals (hold these)

- **No product features.** Tabs, sessions, settings, panes are meerkat's.
  Pelt chrome is an omnibar and back/forward, full stop.
- **No new render glue.** Pelt consumes pelt-live's lib today and the
  cheap-path C1 query object when it lands, like every other host.
- **No Masonry, ever again.** The viewer mode returns on the direct-present
  stack only.

## Phases (done-conditions, not dates)

### V0 — Move the present core into serval (the unlock)

Relocate `serval-winit-host` from mere (`crates/serval-winit-host`) into
serval as **`components/serval-winit-host`** (components, not ports: meerkat
consuming a serval *component* is the established pattern — xilem-serval,
serval-layout, scripted-dom; keep the crate name so consumers re-point paths
only). Re-point meerkat and the orrery bin; mere's workspace drops the local
crate; pelt-desktop gains the dep. Coordination note: this touches meerkat's
imports while window-composition work is in flight — land it between
reshapes, as a single mechanical commit, the same care as the glue
extraction.

**Done when** meerkat and the orrery bin build against the serval-side crate
with zero behavior change, mere's `crates/serval-winit-host` is gone, and a
bare serval clone builds the crate standalone.

### V1 — The viewer mode, static-first, on the modern stack

`pelt --engine static <url-or-file>`: load bytes → `StaticDocument` →
`serval-render`'s `scene_from_layout_dom` pipeline → present via V0's
`serval-winit-host` core. Document *loading* is the genuinely new work, and it is
where `ResourceFetcher` gets its consumer
back: `file://` and `data:` first-party; http(s) behind a returning
`netfetch` feature (netfetcher-backed, off by default, replacing the one
dropped with pelt-viewer — this time wired to a fetcher the contract was
designed for). Scroll wheel + resize; no chrome yet (URL from argv).

**Done when** `pelt --engine static <local file>` renders and scrolls a real
document in a window on the modern present path, `--engine static
https://…` works under `--features netfetch`, and the capabilities printout
matches what the profile actually wires (no aspirational flags).

### V2 — Minimal chrome as xilem-serval views (the public demo)

An omnibar + back/forward built as xilem-serval views over a second document
root, exactly meerkat's separate-roots discipline at 1/20th the size. This
makes pelt the **mere-free public demo** of the xilem-with-a-real-DOM
toolkit story: anyone evaluating serval or xilem-serval gets a runnable
answer in one clone.

**Done when** typing a URL in pelt's omnibar navigates the content root,
back/forward walk a simple history, the chrome root and content root never
see each other's tree, and the whole shell remains small enough to read in
one sitting (pelt stays the thin reference).

### V3 — Headless screenshot mode → the reftest harness (highest engine leverage)

`pelt --engine headless --out <path> <file>`: run the pipeline windowless
(pelt-live's lib already proves GPU-free runs), emit **both** a netrender
scene snapshot (postcard, byte-deterministic — the primary comparison
artifact) and a rasterized PNG (for human eyes). On top of it, a tiny
fixture runner: a directory of `name.html` + `name.scene` (+ optional
`name.png`), compare, report. Every "done when X renders in a reftest" in
the pseudo-element follow-ups, the gc-arena soak, and future layout work
gets its harness here; serval gains its first regression net beyond unit
tests.

**Done when** a fixture directory runs green in one command, a deliberate
layout change turns exactly the affected fixtures red with a scene diff
named, and the pseudo-element follow-ups' shipped slices (`::before`
content, `:checked`) land as the first fixtures.

### V4 — The scripted profile (the content tier's proving ground)

`pelt --engine scripted`: page `<script>` runs through script-runtime-api on
the selected engine (Nova native / Boa wasm-oracle, the serval-wpt
selection pattern) against a `ScriptedDom` document, with the engine's DOM
bindings and the reflector bridge live on a real page. Nothing exercises
full-page scripting end to end today, and the gc-arena plan's G1-G3
(reflector liveness, the dangle contract, collection) want exactly this
workload — a real page holding real reflectors over a long-lived document —
before meerkat's content lane inherits it.

**Done when** a local page with inline script mutates its own DOM and the
mutation renders; `--engine` selects Nova or Boa for the same page; and a
soak page that churns nodes under script runs long enough to feed the
gc-arena plan's G1 liveness probe with real data.

## Open questions

1. **Harness comparison format strictness (V3)** — scene-snapshot equality
   is byte-exact and GPU-free but invalidates on any paint-order change;
   PNG comparison tolerates paint reshuffles but needs a GPU and fuzz
   thresholds. Start scene-primary + PNG-on-demand; revisit if scene churn
   makes fixtures noisy.
2. **Where pelt's queries land (V1/V2)** — pelt-live's free functions today,
   the C1 laid-out-document object when it ships. Adopt C1 in pelt the same
   release it lands so the reference shell demonstrates the cheap path, not
   the expensive one.
3. **Profile honesty** — `EngineProfile`'s capabilities printout predates
   any of this; V1/V4 should derive the flags from what is actually wired
   per profile rather than the static table.
4. **`static_viewer` scaffold fate (pelt-desktop)** — fold into V1's viewer
   or keep as the smoke-shaped probe; decide when V1 touches it.

## Progress

- **2026-06-12** — Plan created, on the heels of the pelt-viewer retirement.
  Role decided: pelt = serval's servoshell (thin reference shell + validation
  entrypoint + reftest harness), meerkat = the product shell. V0 (present-core
  move) is the unlock and the only cross-repo step; V3 is the highest
  engine-development leverage; V4 feeds the gc-arena plan. No code yet.
- **2026-06-12** — **V0 done.** `serval-winit-host` relocated mere → serval
  (`components/serval-winit-host`); meerkat + the orrery bin re-point; all build
  clean, zero behavior change. serval `e075cc5c9c5`, mere `41cb7c6`.
- **2026-06-12** — **Render-driver reform done** (the V1 foundation, V0-shaped).
  Prompted mid-V1-planning by "shouldn't pelt-live be reformed?": making
  `pelt-desktop` consume `pelt-live`'s lib would have been a third consumer of a
  `ports/` probe's render pipeline — the inverted-dependency smell V0 fixes. So
  `pelt-live`'s lib (`render.rs` + `a11y.rs`: ScriptedDom/LayoutDom → Scene + host
  queries + a11y) was extracted into `components/serval-render` (21 tests green,
  incl. the cascade-determinism + host-spine suites), and `pelt-live` **retired**
  (counter bin deleted, lib tests moved with the component). Now the two serval
  host cores — render (`serval-render`) and present (`serval-winit-host`) — are both
  components. meerkat keeps its own copy (deliberate cross-repo insulation, per the
  render-glue-extraction plan). serval `b108fb509ca`. The cascade-offthread probe
  (gitignored mere scratch) re-points locally. V1 now builds on `serval-render`.
