# serval holistic audit (2026-05-29)

A cross-subsystem snapshot: where serval is, where it is going, and the
synergies, contradictions, pitfalls, and sidequests that the per-subsystem
docs each see only a slice of. Grounded against the code and the WPT runner
results, not doc-to-doc. Companion to the per-area plans it cites.

## The one-line thesis

serval is converging two arcs onto one substrate. Arc A is web conformance
(run the real web: testharness, DOM, eventually fullweb). Arc B is
serval-as-host (serval's own DOM as a native Xilem-driven application UI,
the Blitz/Dioxus "chrome as CSS" model;
[2026-05-27_serval_as_host_xilem_serval_plan.md](./2026-05-27_serval_as_host_xilem_serval_plan.md)).
Both ride one DOM (`serval-scripted-dom` + `LayoutDomMut`), one event model
(capture/target/bubble), one render spine (Stylo + box-tree + parley +
netrender). The strategy is that convergence. The test of whether a piece of
work is on-strategy is whether it serves both arcs.

## Grounded state by layer

Verified against the WPT runner and the on-screen demos, correcting probe-era
doc language that has fallen behind the code.

- **Layout: strong.** Block, inline, floats, replaced `<img>`, the planes
  model (Style/Layout/Fragment keyed by `NodeId`), and `IncrementalLayout`
  (attribute change to repaint-vs-relayout damage). Author CSS does cascade
  and lay out: `serval-wpt reftest css/CSS2/floats` pixel-compares real inline
  + linked stylesheets (7 passing), box-shadow reftests render. The stale
  "empty stylist, no CSS applies" framing in some notes describes the e2e
  *test* fixture, not the engine. Not wired: flexbox / grid / positioned
  (taffy supports them; the cascade-to-taffy mapping does not drive them yet).
- **Paint: backgrounds, boxes, images real; text is the gap.** Glyph runs emit
  empty and the netrender translator skips `DrawText`. Pages lay out correctly
  but text does not appear. Highest-leverage rendering gap, and a wiring task
  (parley `Layout` already exists at measure time), not new architecture.
- **Compositor: Windows (DCOMP) and macOS (CALayer) present on hardware;
  Linux Wayland is a shape-locked skeleton** (`present_master` returns
  `Unwired`; dmabuf export + Vulkan timeline sync unwritten). Handoff brief:
  [2026-05-28_wayland_per_surface_presentation_gap.md](./2026-05-28_wayland_per_surface_presentation_gap.md).
- **Scripting tier: a working conformance loop.** Pluggable engines (Nova
  native, Boa wasm/oracle) behind one `ScriptEngine`/`CallCx` trait;
  `script-runtime-api` host surface (event loop, EventTarget, microtasks, DOM
  W0 + reflection + traversal + namespaces); WPT runner phase 3. html/dom
  35,366 subtests passing, dom/nodes 832, dom/lists 100, dom/traversal 32, all
  on Boa. Path: pluggable-engines and WPT-runner plans.
- **Interaction: hit-test + box-model done; focus / affordance / selection /
  activation queries stubbed** (`serval_lane.rs`, probe v1).

## Synergies (the load-bearing ones)

- **The event model is shared by mandate.** The host doc states native
  dispatch (its Gap 2) and the JS-bootstrap capture/target/bubble dispatch
  already in `script-runtime-api/dom.rs` (W0c) must converge, not fork: one
  event model, two entry points (native handlers, JS listeners). Building
  either pulls the other most of the way. This is the strongest synergy and
  the one most at risk of silent divergence.
- **Every DOM method is dual-use.** `insert_before` / `remove_attribute` /
  `remove_child` were added for xilem-serval's `ElementSplice` and are exactly
  the `insertBefore` / `removeAttribute` / `removeChild` the JS DOM surface
  needs. The reflection/traversal/namespace work feeds xilem-serval's Stage 3
  Element/Text split directly.
- **`CallCx` is the reusable engine seam.** A third engine (QuickJS) is
  "implement the trait," and the host surface comes free. The next engine-API
  extension (an exotic/object primitive) unlocks live HTMLCollection/NodeList
  and `dataset` together: one primitive, broad payoff.
- **The WPT runner is free whole-stack verification.** 35k passing subtests
  adversarially validate DOM + events + engines, and cross-check Nova vs Boa,
  making the two-axis (engine vs binding) conformance story legible.
- **netrender is the shared output for everything**: static HTML, scripted,
  xilem-serval chrome, and sibling lanes (Nematic, Scrying).

## Where it sits in the larger frame

- **Profile ladder** (static-html / interactive-html / scripted / fullweb as
  package-witnessed tiers, low tiers carrying no JS engine):
  [2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md).
  Strategically canonical; the P1/P2 mechanics there are historical (the
  script-coupled layout edges were resolved by the 2026-05-20 cut + the planes
  architecture).
- **Hekate lanes** (Nematic / Serval / Scrying):
  [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md).
  Serval has internal tiering; Hekate routes by engine, Serval picks the tier.
- **Ecosystem:** Mere host is Xilem, which is why xilem-serval matters; Scrying
  (system-webview lane) can borrow a web-sys-shaped DOM bridge later
  ([2026-05-26_scrying_dom_bridge.md](./2026-05-26_scrying_dom_bridge.md)).

## Contradictions and tensions

- **Reconciled (engine axis), in docs but not yet code.** The JS-execution
  doc settled six: wasm is Boa+weval (not no-JS); Boa promoted from oracle-only
  to the wasm shipping engine; Nova native-primary; Rhai (app scripting) vs
  Boa/Nova (content JS) is a domain split, not a conflict;
  [2026-05-25_js_execution_strategy.md](./2026-05-25_js_execution_strategy.md).
  Settled as decisions; no weval prototype exists yet.
- **Live: two masters for the DOM.** `serval-scripted-dom` serves both
  web-faithful semantics (WPT) and app-host ergonomics (xilem-serval). The
  host doc's answer is "same engine, separate document/surface authority,"
  but that boundary is asserted, not yet enforced in code. Watch as both arcs
  grow.
- **Doc drift is itself a pitfall.** Probe-era language ("empty stylist, no
  CSS") now lags the code and misleads a fresh reader (it misled an audit
  pass). The profile-ladder doc already flags its own sections as historical.
  The map drifts from the territory in places; prefer runtime checks.
- **Profile ladder vs planes** read as a tension but are complementary (tiers
  = capability witnesses; planes = where mutable render state lives).

## Pitfalls and debt

- **Text-to-pixels** is the headline rendering gap: everything lays out, text
  does not show.
- **Exotic-object engine primitive** gates the next WPT tier
  (HTMLCollection/NodeList/dataset, indexed DOMTokenList). `CallCx` exposes
  only `make_string` / `make_null` / `reflector_for`. A `ScriptEngine`-trait
  extension implemented per backend is the natural next scripting arc.
- **Boa-fork churn** (the icu pin) plus the deeper risk that weval-on-Boa may
  need VM restructuring, not just annotation, against a thin upstream bus
  factor. The strategy doc's own guidance: prototype before committing.
- **Tier gates are manual.** Nothing in CI fails if `serval-static-html`
  accidentally pulls a `script-engine-*` crate. The package-witness doctrine
  rests on an unautomated check. Add `cargo tree` gates to CI.
- **Nova is native-only by construction** (`Value == usize`, `usdt`), so the
  wasm-JS story depends on Boa. Fine for native-first; it is an architectural
  wall, not a preference.
- **Dead-on-disk inheritance**: OHOS media backend and unused `shared/`
  trait crates (`storage`, `bluetooth`, `background_hang_monitor`). Harmless,
  but clutters the map; a cleanup pass would help.

## Sidequests

- **Take (cheap, high-value):** wire text glyphs to pixels (makes rendering
  visible for every consumer); delete dead inheritance crates; add the
  tier-gate CI check (protects the core doctrine).
- **Take when forced (genuine engine-completeness the host doc names):** form
  controls and DOM-to-AccessKit; both serve xilem-serval and fullweb.
- **Resist (premature):** the weval/wasm-JS arc (prototype-gated, thin bus
  factor, no consumer yet); Wayland (platform-gated, needs a live session);
  full per-tag HTML element interfaces (large surface, modest WPT yield versus
  the exotic-object primitive). And the host doc's standing warning: when the
  GUI wobbles, do not reach for "architecture 2" (serval chrome on top of a
  host framework, two of everything); go to architecture 3.

## Suggested sequencing

Ordered by service-to-both-arcs, which is the on-strategy test:

1. **Shared event-model substrate** (native dispatch + listener registry,
   converged with the JS dispatch algorithm). Both arcs are blocked on it and
   it is the synergy most at risk of forking.
2. **Text rendering** (glyph runs to pixels). Makes the whole stack visible.
3. **Exotic-object `CallCx` primitive.** Unblocks the next WPT tier and live
   collections for xilem-serval in one move.

Each serves Arc A and Arc B at once, which is why they sort to the top.

## Receipts

- WPT (Boa): html/dom 35366/49556, dom/nodes 832/4739, dom/lists 100/189,
  dom/traversal 32/48 (run via `serval-wpt testharness <subset>`).
- Reftest (real CSS): `serval-wpt reftest css/CSS2/floats` 7 passing pixel
  compares; box-shadow reftests render.
- On screen: `pelt --windows-present-surfaces-smoke` and
  `--macos-present-surfaces-smoke` exit 0 on hardware; `pelt-live-counter`
  presents a reactive counter via xilem-serval.
