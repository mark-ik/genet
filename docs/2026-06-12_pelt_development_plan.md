# Pelt Development Plan — serval's reference shell

**Date**: 2026-06-12
**Status**: In progress. **V0 done** — the present core moved into serval as
`components/serval-winit-host`. **Render-driver reform done** — the host
render-driver extracted into `components/serval-render` and `pelt-live` retired
(the V0-shaped move that cleared V1's foundation; see Progress). **Reconciled
2026-06-14** (per-phase audit against the code): **V0–V2, V4, V5 done; V3 mostly
done** (the reftest harness + its fixtures shipped, one viewport fixture + PNG
rasterization open); **V6 gated** (the standalone surface lib + tile-tree contract
are done, but meerkat embedding awaits the `external_texture` element in
xilem-serval and the meerkat-side wiring). The doc had been steering by its
authoring-day "V1 is next" status while V2–V5 quietly shipped; the per-phase
**Status** lines below now track reality. Next real engine-development leverage:
V3's PNG/raster lane and the missing overhang fixture, then V6's gate
(external-texture element) when its other consumers (scrying, input-spine) pull it.
**Role statement (revised 2026-06-12, with Mark):** pelt is two things over
one lib. (a) **serval's servoshell** — the minimal reference browser that
proves the engine standalone, drives engine development without mere's graph
machinery, and is what an outside contributor clones and runs. (b) **The
standalone-capable browsing surface mere embeds**: a tile-tree browser
(splits + tab-stacks of serval documents) built as a host-loop-sheddable
*surface lib*, so the same surface runs under pelt's own winit loop or as
meerkat's workbench pane — the orrery-host pattern's second instance, and
the pressure vessel where the browsing surface hardens standalone before
mere consumes it (the Strophe-for-audio shape). meerkat remains the product
shell (graph, sessions, comms); the pelt *bin* stays thin so the
browsers-are-cheap-to-assemble demonstration survives the module growing
underneath it. The roles just became reachable: the xilem fork is a git dep
(bare clones build), Masonry left the tree with pelt-viewer's retirement
(2026-06-12, workspace audit snapshot), the render/present cores are both
serval components, and the smoke suite already makes pelt the validation
entrypoint.
**Related**: the pelt-viewer retirement note
(`2026-05-16_workspace_audit_snapshot.md`, 2026-06-12 update); mere's host
cheap-path plan (C1's laid-out-document query object is pelt's eventual query
seam too); the gc-arena DOM plan (V4 is its first real scripted workload);
the pseudo-element follow-ups (every done-condition there wants V3's reftest
harness); mere's window-composition plan (its P2-companion input spine and
pane model are what V6 plugs into, and its workbench pane is V6's
destination).

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

## Non-goals (hold these; revised 2026-06-12 with the module charter)

- **No graph, no sessions, no comms, no product chrome.** Those are
  meerkat's. *(Revision: tabs and splits are now in pelt's charter — V5's
  tile tree — superseding the original "tabs/panes are meerkat's" line. The
  line moves, the principle stays: pelt presents documents; mere owns
  meaning.)*
- **No new render glue.** Pelt consumes `components/serval-render` today and
  the cheap-path C1 query object when it lands, like every other host.
- **No papering over engine gaps.** When pelt hits a rendering gap, the fix
  lands in serval-layout as the spec's mechanism and the host change is
  limited to feeding inputs to it — see the
  [viewport & root standards scope](2026-06-12_viewport_root_standards_scope.md)
  (the document-scroll family, fixed-positioning attachment, UA default
  actions). If a fix only works for pelt, it isn't the fix.
- **No Masonry, ever again.** The viewer mode returns on the direct-present
  stack only.

## Design rule (added 2026-06-12): lib-first, bin-as-shell

V1 onward, the viewer is built as a **surface lib** from the first commit —
the orrery-host contract shape (`frame(w, h) -> (Scene, needs_redraw)` +
semantic input + resize), with the pelt bin a thin winit shell over it via
`serval-winit-host`. This is what makes V6's host-loop shedding a
non-event instead of a retrofit: meerkat hosts the same lib the bin wraps,
exactly as it hosts the `Orrery`. Costs nothing now; expensive to bolt on
later.

## Phases (done-conditions, not dates)

### V0 — Move the present core into serval (the unlock)

**Status: Done** (2026-06-12) — `serval-winit-host` relocated mere → serval; see
Progress.

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

**Status: Done** (2026-06-14) — `pelt --engine static <url-or-file>` loads file://,
a bare path, and `data:` (percent-encoded + base64 via the spec parser), renders +
scrolls with engine-side document scroll; http(s) loads under `--features netfetch`
(netfetcher-backed, offline mockito test); the capabilities printout is honest.

`pelt --engine static <url-or-file>`: load bytes → `StaticDocument` →
`serval-render`'s `scene_from_layout_dom` pipeline → present via V0's
`serval-winit-host` core. Document *loading* is the genuinely new work, and it is
where `ResourceFetcher` gets its consumer
back: `file://` and `data:` first-party; http(s) behind a returning
`netfetch` feature (netfetcher-backed, off by default, replacing the one
dropped with pelt-viewer — this time wired to a fetcher the contract was
designed for). Scroll wheel + resize; no chrome yet (URL from argv).

**V1's engine prerequisite (found 2026-06-12): document scroll is a
serval-layout feature, not host code.** A page taller than the window must
scroll with zero CSS via root → viewport overflow propagation (both halves,
the canvas-background sibling), with `position: fixed` gaining real viewport
attachment in the same change (today `Fixed` ≡ `Absolute`,
`paint_emit.rs:418`, a loud regression once scroll exists). Wheel input
routes through the shared default-action helper, not pelt-local scroll math.
Full case family + the engine model rules:
[viewport & root standards scope](2026-06-12_viewport_root_standards_scope.md).

**Done when** `pelt --engine static <local file>` renders and scrolls a real
document in a window on the modern present path **with document scroll
implemented engine-side per the standards scope (no root-overflow-container
hack)**, `--engine static
https://…` works under `--features netfetch`, and the capabilities printout
matches what the profile actually wires (no aspirational flags).

### V2 — Minimal chrome as xilem-serval views (the public demo)

**Status: Done** — omnibar navigation of the content root, back/forward over a
simple history (`Vec<String>` + position, with forward-truncation), strict
two-root separation (the chrome reaches content only via `ChromeIntent`), thin
shell (`chrome.rs` ~464 LOC GPU-free + `chrome_viewer.rs` ~415 LOC windowed),
tested. `--features chrome`; `pelt --chrome <url> --strip <side>`.

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

**Status: Mostly done** — `pelt --engine headless --reftest <dir>` runs a fixture
directory green in one command, a layout change reds the affected fixture with a
named scene diff (`first diff at line N`), and `--bless` (re)writes snapshots.
Shipped fixtures: `before-content` (`::before`), `checked` (`:checked`), and the
viewport family `document-scroll`, `overflow-hidden-root`, `fixed-under-scroll`,
`percent-height`, `scrollable-overflow-overhang` (the abs-pos-overhang case,
`a56882c177d`; scroll sidecars drive the scrolled ones). The fixture set is now
complete. **Open:** the rasterized-PNG lane only (scene-snapshot is the primary
artifact; PNG is a documented offscreen-readback follow-up).

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
content, `:checked`) land as the first fixtures — followed by the
viewport-family fixtures from the standards scope (document scroll
root- and body-propagated, `overflow: hidden` on root, fixed-vs-absolute
under scroll, the %-height chain, scrollable-overflow with an abs-pos
overhang).

### V4 — The scripted profile (the content tier's proving ground)

**Status: Done** — a local page's inline `<script>` mutates its own DOM and the
mutation renders; `--js boa|nova` selects the engine (Nova behind
`--features scripted-nova`); the **GC tick auto-fires at frame cadence** (the
viewer's render pump → `Runtime::collect_garbage`, before layout) and the
`gc_soak_bounds_memory` soak (120 frames × 50-node churn) holds memory bounded —
closing the gc-arena plan's two carve-outs (the explicit→auto GC flip and the
collection soak). **Open:** external `<script src>` (deferred; inline-only by
design). `--features scripted`.

`pelt --engine scripted`: page `<script>` runs through script-runtime-api on
the selected engine (Nova native / Boa wasm-oracle, the serval-wpt
selection pattern) against a `ScriptedDom` document, with the engine's DOM
bindings and the reflector bridge live on a real page. Nothing exercises
full-page scripting end to end today, and the gc-arena plan's G1-G3 (reflector
liveness, the dangle contract, the mark-sweep collector) are **landed
mechanism-complete (2026-06-12), tested, and waiting on exactly this workload
to validate them** — a real page holding real reflectors over a long-lived
document — before meerkat's content lane inherits it. V4 is specifically where
the GC tick (`Runtime::collect_garbage`) gets its first real frame-cadence
caller (today it's an explicit, not-yet-auto-fired call by design) and where
the collection soak runs.

**Done when** a local page with inline script mutates its own DOM and the
mutation renders; `--engine` selects Nova or Boa for the same page; and a
soak page that churns nodes under script drives `Runtime::collect_garbage` at
the frame cadence (the moment to flip the GC tick from its explicit call to
auto-firing — gc-arena plan carve-out #1) and confirms the gc-arena plan's
bounded-memory + no-collect-pause done-condition under a real workload (its
carve-out #2 soak). Both of that plan's remaining carve-outs close here.

### V5 — The tile tree (the surface grows; logically follows V2, may interleave with V3/V4)

**Status: Done** — pelt standalone splits the window, opens documents in tabs
per pane, drags a tab between stacks (drop onto a tab bar merges; onto a tile's
content splits on the nearest edge), and closes tiles (empty stacks collapse,
single-child splits flatten) — all driven through the serval-side tile-tree
contract (`pelt-core/tile.rs`: `TileTree` / `TileEvent` / `ContentSource`, the
reference `apply` reducer), the only seam, with the bin holding no tile logic the
lib doesn't expose. 13 contract + 6 surface + 5 headless-driven-input tests.
`--features tiles`. (The "may interleave" was accurate — it shipped alongside
V3/V4.)

The surface lib gains splits + tab-stacks of documents: per-tile document
lifecycle (N documents live at once), per-tile history, tab activation /
close / drag-between-stacks, divider resize — rendered as xilem-serval flex
DOM (platen-view already proved the rendering shape; the *model* is the new
work). The model is defined against a **plan-shaped tile-tree input
contract** that lives serval-side (near pelt-core): "here is a tree of
splits and tab-stacks; here is each tile's content source" in, tile events
(activated / closed / dragged / resized) out. Standalone pelt populates the
contract from its own simple state. Deliberately *presentation-grade*: no
graph-capable arrangement, no persistence beyond the running shell — forme
remains the arrangement truth on the mere side, and maps onto this contract
rather than being duplicated by it (platen's `tree_projection` already
compiles forme → `WorkbenchPlan` = splits of tab-stacks, so the mapping is a
projection, not a second authority).

**Done when** pelt standalone can split the window, open documents in tabs
per pane, drag a tab between stacks, and close tiles — with the tile tree
driven entirely through the contract (the bin holds no tile logic the lib
doesn't expose).

### V6 — Shed the loop: pelt-surface as meerkat's workbench pane (the module)

**Status: Gated (serval side done; meerkat wiring next)** — the serval half is
ready: the standalone pelt surface lib works unchanged (V5), the tile-tree contract
is the only seam (`pelt-core`'s `ContentSource` already names the
`ExternalTexture(key)` lane), and gate (1) is now **resolved** — the
**`external_texture` element view in xilem-serval** landed (`a8832e2762a`), the
shared primitive the scrying plan and the input-spine companion also wanted (an
`<external-texture key>` replaced leaf that paint emits as a `DrawExternalTexture`
compositor pass). The two remaining gates are pure meerkat-side wiring (no new
serval components): (2) **meerkat hosting
`TileSurface`** in its workbench pane (today it renders tiles through platen-view's
own `WorkbenchScene`, not the pelt surface; meerkat doesn't yet depend on
`pelt-core`/`pelt-desktop`); (3) the **`tree_projection` → `TileTree` mapping** in
meerkat's reconcile loop (platen's `tree_projection` produces a `WorkbenchPlan`
but nothing maps it onto the contract). The `WorkbenchScene` and `TileSurface`
stay distinct by design — mere projects forme onto the simpler contract, not a
union.

The embedding step. meerkat hosts the V5 surface lib as a pane: mere's
platen maps the forme arrangement through `tree_projection` onto the V5
contract; per-tile content arrives as either a **serval content-root
subtree** (documents) or an **`external_texture(key)` element** (constellation
actor textures, scrying WebViews — the routing distinction
`SurfaceContractMode::CompositedTexture` already names). The pelt bin and the
meerkat pane wrap the *same lib*; neither knows the other exists.

**Gated on** mere's window-composition P2+ (panes resolve everywhere) and
the external-texture element view in xilem-serval (the same missing
primitive the scrying plan and the input-spine companion already point at —
this is now the fourth consumer waiting on it). This phase is also the
second instance of the orrery-host pattern, which is the moment to write
down the **pane-module contract** generally (standalone-or-hosted surface:
frame / input / resize / content-source), since roster/gloss/apparatus want
the same shape under the window-composition pane model.

**Done when** meerkat's workbench pane renders through the pelt surface lib
with forme-projected tiles and mixed content (a serval document tile beside
an actor-texture tile), the standalone pelt bin still works unchanged from
the same lib, and the tile-tree contract is the only seam between them.

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
5. **The tile-tree contract's home and owner (V5/V6)** — serval-side, near
   pelt-core, defined *before* a second implementation exists. platen maps
   onto it; pelt-surface implements it; drift between them is the risk to
   guard (the contract is presentation vocabulary only — if it starts
   wanting graph concepts, it is drifting toward forme and should stop).
6. **The dual-role tension** — the thin-demo bin and the capable module will
   tug in review. The lib/bin split absorbs most of it; the tell to watch
   for is bin-side logic the lib doesn't expose (forbidden by V5's
   done-condition) or lib features only meerkat could ever use (those
   belong mere-side).

## Progress

- **2026-06-12** — Plan created, on the heels of the pelt-viewer retirement.
  Role decided: pelt = serval's servoshell (thin reference shell + validation
  entrypoint + reftest harness), meerkat = the product shell. V0 (present-core
  move) is the unlock and the only cross-repo step; V3 is the highest
  engine-development leverage; V4 feeds the gc-arena plan. No code yet.
- **2026-06-12** — **V0 done.** `serval-winit-host` relocated mere → serval
  (`components/serval-winit-host`); meerkat + the orrery bin re-point; all build
  clean, zero behavior change. serval `e075cc5c9c5`, mere `41cb7c6`.
- **2026-06-12** — **Charter revised (with Mark): pelt grows the module role.**
  pelt's ideal end-state is a self-sufficient evolution of the workbench for
  mere/meerkat — a tile-tree browser that sheds its host loop to plug into
  meerkat. Distance assessed as three gaps: host-loop shedding ≈ zero (the
  orrery-host pattern + the now-componentized render/present cores; adopted as
  the lib-first design rule), the tile tree = modest (V5, presentation-grade
  model over a serval-side plan-shaped contract; forme stays truth, platen's
  `tree_projection` maps onto it), content unification = the external-texture
  element again (V6's gate, now its fourth waiting consumer). Non-goals
  revised: tabs/splits enter the charter; graph/sessions/comms stay meerkat's.
  V6 is also where the generalizable pane-module contract (standalone-or-
  hosted surface) gets written down, since roster/gloss/apparatus want the
  same shape.
- **2026-06-14** — **Phase-status reconciliation** (per-phase audit against the
  code, file:line-verified, five parallel auditors). The doc had drifted: it read
  as if V1 were the frontier while V2–V5 had quietly shipped. Reconciled to: V0,
  V1, V2, V4, V5 **done**; V3 **mostly done** (harness, `::before`/`:checked`, and
  viewport fixtures all present; only the scrollable-overflow-overhang fixture and
  the PNG raster lane remain), V6 **gated** on the `external_texture` element
  (xilem-serval) + meerkat wiring + the `tree_projection`→`TileTree` map. Per-phase
  **Status** lines added above; no code changed. Notable finds: the GC tick already
  auto-fires at frame cadence (V4 carve-outs closed); the tile-tree contract
  (`pelt-core/tile.rs`) already names a third `Settings` content lane beyond the
  plan's two; V6's blocker is "meerkat hasn't plugged the surface in," not "the
  surface doesn't exist."
- **2026-06-14** — **V1 done.** The static viewer was already wired end to end
  (`pelt --engine static <url>` → `LoadedDocument` → `serval-render` →
  `serval-winit-host`, with engine-side document scroll from the viewport scope);
  this session closed the loading gaps. `data:` decoding moved to the spec parser
  (`data_url::DataUrl`), gaining base64 bodies; http(s) landed behind a `netfetch`
  feature that drives the netfetcher engine (the host-owns-networking pattern,
  mirroring serval-wpt's `fetch()` wiring — path dep + tokio bridge over the sync
  `ResourceFetcher`, offline mockito test). The wheel also now routes through the
  engine's `scroll_at`, so nested `overflow:scroll` containers scroll under the
  cursor. serval `67c4a8acf93` (netfetch), `016333bd1a9` (base64 + help text),
  `7874e5a1297` (nested-scroll wheel).
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
