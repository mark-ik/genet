# genet holistic audit (2026-06-02)

A cross-subsystem snapshot: where genet is, where it is going, and the
synergies, contradictions, pitfalls, and sidequests the per-area docs each see
only a slice of. Grounded against the code, the commit log, and the test
suites, not doc-to-doc. Refreshes and supersedes the
[2026-05-29 audit](./2026-05-29_genet_holistic_audit.md); companion to the
[two-lanes operationalization](./2026-05-29_genet_two_lanes.md).

## The one-line thesis (unchanged)

genet converges two arcs on one substrate. Arc A is web conformance (run the
real web: testharness, DOM, eventually fullweb). Arc B is genet-as-host
(genet's own DOM as native Xilem-driven app UI, chrome as CSS). Both ride one
DOM (`genet-scripted-dom` + `LayoutDomMut`), one event model
(capture/target/bubble + a shared cancellation contract), one render spine
(Stylo + box-tree + parley + netrender). The on-strategy test of any work is
whether it serves both arcs.

## Corrections to the 2026-05-29 audit (read this first)

The prior audit warned that its own map could lag the territory. It did, on at
least six claims, several already false on its write date. Anyone planning off
the old doc should treat these as overturned:

- **"text is the gap, the translator skips `DrawText`" is wrong.** Text is
  wired end to end and was before the audit. `paint_emit::emit_inline_content`
  emits real parley glyphs (paint_emit.rs:760-871), `paint_list_render`'s
  `DrawText` arm calls `push_glyph_run_full` (lib.rs:512-540, shipped in netrender
  802f9cc4f, 2026-05-26), the vello rasterizer draws them. meerkat and
  orrery-host render text on screen.
- **"flexbox / grid / positioned not wired" is wrong.** The box tree dispatches
  `Display::Flex/Grid` to taffy (box_tree.rs:424-470), with full
  `LayoutFlexboxContainer`/`LayoutGridContainer` impls; positioned forwards
  `position()`/`inset()` and is unit-tested. This was already true at commit
  f51b86c36a2 (2026-05-25), four days before the audit.
- **The "exotic-object primitive gates the next WPT tier" pitfall + sequencing
  item is done.** Live HTMLCollection/NodeList/DOMTokenList/dataset shipped on
  the audit day (f20a692f05f, bdfe08d6e8f, 2026-05-29) via a JS `Proxy` route in
  the bootstrap, deliberately not the per-backend trait extension the audit
  recommended. The engine-API surface is unchanged.
- **"Boa-fork churn (the icu pin)" debt is resolved** (2fad3c1d3b1, 2026-06-01:
  synced Boa to upstream 1.0.0-dev, dropped the icu fork-patch).
- **"`storage` is dead-on-disk" is wrong.** `servo-storage-traits` is a live
  transitive dep of the default build (pelt to pelt-viewer to servo-paint to
  servo-constellation-traits to servo-storage-traits). A naive "delete dead
  crates" pass following the old audit would break the build.
- **The event-model convergence is built, not future work** (see below).

The lesson stands and is the audit's own: prefer runtime checks; doc-drift here
is real and biting.

## Grounded state by layer

- **Layout: strong, including flex / grid / positioned.** Block, inline,
  floats, replaced `<img>`, the planes model, `IncrementalLayout`, and the
  box-tree to-taffy dispatch for flex/grid/positioned all work. Positioned was
  hardened 2026-06-02 by ancestor-CSS-transform propagation to abs-pos stacking
  layers (3ecb09823a7, Orrery 1A). `IncrementalLayout` now also emits a paint
  list (79e3023c5a3, Orrery 3c). Caveat: flex/grid are reachable but have no
  genet-owned unit/reftest; correctness rests on taffy upstream plus the
  downstream consumers (meerkat flex, orrery abs-pos+transform) exercising them.
  `stylo_taffy` is pinned to a pre-release alpha, a high-blast-radius pin.
- **Paint: backgrounds, boxes, images, strokes, and text all render.** The
  headline gap is closed. Remaining gaps are narrower: stroke cap/join/dash not
  yet honored (solid butt strokes; lib.rs:465-479), text-decoration partial
  (underline drawn as a rect, others unemitted), inset box-shadow deferred,
  `ClipKind::Path` falls back to no-clip, the exotic mix-blend-modes fall back to
  Normal. One translator (`paint_list_render` in netrender) plus one rasterizer
  (vello) serves every consumer.
- **Compositor: Windows (DCOMP) and macOS (CALayer) present on hardware,
  including per-`SurfaceKey` paths; Linux Wayland is a shape-locked skeleton.**
  `WaylandSubsurfaceBackend::present_master` returns `Unwired`
  (compositor_wayland.rs:148-152); dmabuf export and a Vulkan/Metal timeline
  synchronizer are unwritten (only `Dx12FenceSynchronizer` exists). One nuance
  the old audit understated: macOS per-surface IOSurface presentation has landed,
  so the per-surface contract is proven on two of three platforms. Handoff:
  [2026-05-28_wayland_per_surface_presentation_gap.md](./2026-05-28_wayland_per_surface_presentation_gap.md).
- **Scripting: a maturing conformance loop on two engines.** Pluggable
  `ScriptEngine`/`CallCx`/`ScriptEngineLive` (Nova native-primary, Boa
  wasm/oracle), both backends implementing all three. Recent: the regex shim was
  retired (Nova `RegExp` moved to `regress` for full ECMAScript backtracking,
  a5a3e51e711); a six-bug WTF-8/UTF-16 string-indexing panic family was closed
  (c74396822c0, 410ca891d6b); DOM binding globals `HTMLElement`/`customElements`/
  `frames` added (c4ba29aa93f). Across the whole `dom/` tree both engines now
  show zero panics, differing only in a thin subtest-count tail. WPT (2026-06-02):
  dom/nodes Nova 1640/5280, Boa 1646/5325 (the old 832/4739 is obsolete);
  html/dom 35,366/49,556 as last reported.
- **WebGL / ESSL: a large new arc the old audit did not see.** `webgl-essl` is a
  pure-Rust ESSL to SPIR-V to WGSL shader-compiler frontend (an
  ANGLE-shader-translator replacement for the fullweb WebGL tier). It dominates
  post-audit history (roughly 39 of ~137 commits, 2026-06-01..06-02). No live
  consumer yet: no canvas binding, CTS conformance (Step 7) and the production
  swap (Step 8) not started. On-strategy verdict below.
- **DOM: the dual-use substrate is real and confirmed.** `ScriptedDom` (one
  561-LOC `NodeId` arena, `LayoutDom` + `LayoutDomMut`) backs both arcs:
  `insert_before`/`remove`/`set_attribute` are simultaneously xilem-serval's
  `ElementSplice` sinks and the JS `insertBefore`/`removeChild`/`setAttribute`.
  `customElements` is a record-only stub (define/get/whenDefined, no upgrade;
  custom-elements 3/2807).
- **Genet-as-host (Arc B): now real, not a counter demo.** Three mere crates
  stand on `xilem-serval`: **meerkat** (a browser chrome over the reused
  graphshell chrome view-models: nav/history, content-root seam, omnibar
  suggestions, command palette), **orrery-host** (an interactive force-directed
  graph canvas: pan/zoom/inertia, node drag via gyre pin/unpin,
  node/marquee/edge selection, a pre-materialized DOM pool driven on the
  `RepaintOnly` path), and **genet-winit-host** (the shared winit+netrender
  present stack). Critically, host-lane work drove shared-spine capabilities the
  content lane also gets: the 1A abs-pos transform fix, `IncrementalLayout::emit_paint_list`,
  and the caret content-box fix. This is the convergence thesis proven in code,
  not asserted.
- **Event model: converged, the old audit's #1 risk largely retired.** Both
  dispatchers now satisfy one propagation/cancellation contract (8127847b77d,
  2026-06-01). JS side has capture/target/bubble, stopPropagation,
  stopImmediatePropagation, preventDefault, once, passive, composedPath,
  eventPhase, window-as-target. Native side (xilem-serval) gained a shared
  `Propagation` cell embedded in `PointerClick`/`KeyEvent` that the dispatch loop
  honors and a caller can read. A shared scenario table asserted in both crates
  is the anti-drift guard; four conformance tests pass (2 JS engines, 2 native).
- **Interaction: hit-test + box-model done; the host-cancellation seam is wired
  but unconsumed** (pelt-live dispatches events but never reads
  `default_prevented()`).

## Synergies (the load-bearing ones)

- **Host-lane work is advancing the shared spine.** Building the orrery forced
  the CSS-correct 1A transform fix and `IncrementalLayout::emit_paint_list`;
  meerkat exercised flex; the caret fix corrected glyph emit for everyone. Arc B
  served Arc A, which is the strongest live evidence the convergence is genuine.
- **Every DOM method is dual-use** (one set of `LayoutDomMut` methods, two
  callers: `ElementSplice` and the JS surface).
- **The event scenario table is a reusable anti-fork artifact**: one source of
  truth the WPT-facing JS column and the host-facing native column both assert.
- **One translator + one rasterizer for all lanes** (static HTML, scripted,
  xilem-serval chrome, orrery underlay), with `composite_paint_layers` merging
  multiple producers via collision-free key namespacing.
- **The WPT runner is free whole-stack verification**, now cross-checked across
  Nova and Boa.
- **`CallCx` is the reusable engine seam**, and the exotic-object payoff was
  realized via `Proxy` with zero trait growth.

## Where it sits in the larger frame

- **Two development lanes, one spine** is the canonical operationalization
  ([2026-05-29_genet_two_lanes.md](./2026-05-29_genet_two_lanes.md)), distinct
  from Hekate engine-routing lanes (Nematic / Genet / Scrying).
- **Profile ladder** (static-html / interactive-html / scripted / fullweb as
  package-witnessed tiers, low tiers carrying no JS engine) remains
  strategically canonical
  ([2026-05-12_genet_profile_ladder_plan.md](./2026-05-12_genet_profile_ladder_plan.md)).
- **Ecosystem:** Mere host is Xilem, which is why xilem-serval matters and why
  meerkat/orrery-host exist; Scrying can borrow a web-sys-shaped DOM bridge later.

## Contradictions and tensions

- **The dominant investment is single-arc.** webgl-essl (about 39 commits in
  ~28h) is fullweb-tier WebGL with no consumer yet; host chrome and the orrery
  paint via vello/netrender, not WebGL. By the audit's own serves-both-arcs
  test it is content-lane-only, and it pulls effort ahead of the spine items
  (event-model seam, CI/tier-gate) the strategy ranks highest. Either a
  deliberate fullweb-conformance incubation or scope creep; name the call.
- **The two-masters DOM boundary is enforced by the embedder, not the
  substrate.** meerkat instantiates two independent `ScriptedDom` arenas
  (chrome-root vs content-root, 2cec445) so neither root sees the other's tree.
  But `genet-scripted-dom` has no type-level or runtime fence; handing both arcs
  one `Rc<RefCell<ScriptedDom>>` would silently merge them. The old audit's
  "watch as both arcs grow" survives at the substrate level.
- **The event anti-drift guard is convention, not enforcement.** The two-crate
  scenario table is kept in sync by reviewer discipline plus matching doc
  comments; the crates are separate dep islands, so nothing in CI fails if one
  column changes without the other.
- **Nova is native-only by construction** (`Value == usize`), so the wasm-JS
  story depends on Boa. An architectural wall, not a preference.
- **Doc-drift is itself a recurring failure mode** (six overturned claims
  above). The map drifts; trust the code.

## Pitfalls and debt

- **There is no CI in the genet repo.** The only tier gate is a manual
  PowerShell script (`support/profile-gates/check-static-html.ps1`) covering one
  tier. The package-witness doctrine the whole profile ladder rests on is
  unautomated.
- **The native cancellation seam has no host consumer**, so Arc B's event payoff
  (gating form activation / drag / caret default) is wired and unit-tested but
  unverified end to end.
- **The content-root engine does not exist.** meerkat's content root shows
  synthesized HTML (`build_content_dom` echoes the URL); no fetch/parse/script
  engine is wired behind the seam.
- **Dead-on-disk crates remain** (OHOS media, `shared/bluetooth`,
  `shared/background_hang_monitor`), undeleted. Exclude `storage` from any
  cleanup (it is live).
- **`customElements` has no upgrade/lifecycle**; flex/grid have no genet-owned
  test; `URL` is unimplemented; Wayland is unwired and needs a live session.

## Sidequests

- **Take (cheap, dual-arc, protective):** stand up any CI harness, then promote
  the tier-gate check into it (invert the grep to fail on a match), with
  interactive/scripted-tier variants; delete the dead OHOS/bluetooth/bhm crates
  (not storage); have a host read `default_prevented()` so Arc B's cancellation
  payoff is live; a runtime/type fence in `genet-scripted-dom` to make the
  chrome/content authority boundary un-violable.
- **Done (drop from the backlog):** text-to-pixels; the exotic-object primitive;
  the Boa icu pin.
- **Resist (premature):** webgl-essl integration ahead of a canvas consumer
  (defer integration, or accept it as a deliberate incubation lane with the
  mozangle differential oracle); the full per-tag HTML interface tree; Wayland
  (platform-gated). The standing warning still holds, and meerkat/orrery now
  embody its resolution: when the GUI wobbles, do not build "architecture 2"
  (genet chrome on a host framework, two of everything); go to architecture 3,
  which is what the host crates are.

## Suggested sequencing

Ordered by service-to-both-arcs:

1. **Cash in the event-model seam + add a CI harness.** The convergence is
   built; the remaining value is a host reading cancellation (Arc B) and a CI
   tier-gate protecting the package-witness doctrine (both lanes). These are the
   highest-leverage, lowest-cost moves now that the old #1 (event convergence)
   is done.
2. **A content-root engine behind meerkat** (fetch + parse + script into the
   content authority), turning the host's content seam into a real page.
3. **Decide webgl-essl's strategic place** (consumer-driven integration vs
   incubation), so the dominant workstream is consciously on- or off-spine.

## Receipts

- WPT: dom/nodes Nova 1640/5280, Boa 1646/5325 (`docs/2026-06-02_nova_wtf8_indexing_fixes.md`);
  html/dom 35,366/49,556 as last reported; whole `dom/` tree zero panics on both
  engines (`docs/2026-06-02_wpt_dom_sweep_and_binding_globals.md`); test262
  RegExp+String 244 improvements, 0 regressions.
- Tests run this pass: `genet-layout` box_tree 9/9; `paint_list_render` 10/10;
  event conformance 4/4 (`dom_node_events` on Boa + Nova; native
  `stop_propagation_halts_the_bubble_walk` + `prevent_default_is_visible_to_the_caller`).
- On screen: meerkat (browser chrome) and orrery-host (interactive
  force-directed graph canvas) render and interact on hardware this session;
  `pelt --windows-present-surfaces-smoke` / `--macos-present-surfaces-smoke`
  exit 0; macOS per-surface IOSurface path landed.

## Method note

This refresh re-grounded each subsystem against code + commits via a
verification fan-out (layout, paint, compositor, scripting, DOM, event-model,
larger-frame done as structured findings; webgl and genet-as-host grounded from
the commit log and the mere host crates first-hand). The two-masters and
event-model tensions are the ones to watch as both arcs grow.
