# Grand audit: serval + netrender (WPT, capabilities, wasm/async, architecture)

**Date:** 2026-06-24
**Status:** audit (code-grounded). Spins out four scoped plans + three extensions; see the index at the foot.
**Method:** an 18-agent fan-out read the actual code (not just docs) and cited everything at file:line; the WPT baselines were re-measured against the repo, an adversarial pass verified each candidate lever, and a completeness critic checked the drafts. Numbers below are from the repo's logs/docs or an agent re-measure, attributed inline.
**Scope:** serval (engine), netrender (renderer), meerkat (the Mere host, `repos/mere/crates/meerkat`), and the sibling crates errand / netfetcher / tincture / wgpu-{graft,scry,weld}.
**Headline correction:** the conformance baselines in circulation are stale by 5x to 40x. Floats is 42/197 not 7/197, css-backgrounds 334/1325 not 15/1326, normal-flow 462/1044 not 1/1045. DOM core is panic-free on both Nova and Boa. serval is well past the dangerous early phase; the remaining distance is dominated by **harness plumbing**, then the **CSS layout long tail**, in that order.

**Related:** the CSS conformance plan (`2026-05-31_css_rendering_conformance_plan.md`), the WPT runner plan (`2026-05-26_wpt_runner_plan.md`), the wasm-enablement plan (`2026-06-06_wasm_enablement_and_crate_rename_plan.md`), the element-view + scripted-tier plan (`2026-06-16_element_view_and_scripted_tier_plan.md`), the holistic + capability audits (`2026-06-02_serval_holistic_audit.md`, `2026-06-14_engine_capability_audit.md`); netrender notes (`repos/netrender/netrender-notes/PROGRESS.md`); the mere-side meerkat plan spun out below.

---

## 1. The honest distance to 100% WPT

"100% WPT" is not a real target. WPT is a moving cross-vendor superset of nearly the whole platform (~56.5k tests / ~1.8M subtests, growing weekly); no shipping engine is at 100%. Serious engines steer by **absolute passing-subtest count over time**, not a percentage (Servo went 30%->62% over ~2.5y; Ladybird reports ~2.07M passing subtests). serval should adopt that metric.

Three axes the stale baselines conflate, separated:

- **Language conformance (test262, NOT in WPT).** Nova ~80% (40,515/50,733; ~93% once Temporal+Intl excluded), Boa ~94% (external, the oracle). WPT excludes ECMAScript, so this neither bounds nor predicts WPT. Keep Nova-80 and Boa-94 distinct.
- **Engine correctness (CSS reftests + DOM testharness).** Real current state: floats 42/197, normal-flow 462/1044, css-backgrounds 334/1325, css-images 172/713, css-writing-modes 219/1829, css-multicol 103/923, css-tables 61/402; dom/nodes ~61/3341, custom-elements 3/2807. CSS is the long pole by raw volume (the local checkout is 46,907 CSS files vs 11,516 html / 629 dom / 547 fetch).
- **Harness runnability (gates whole directories before any engine work counts).** This is the binding constraint. The runner is a hand-rolled directory walk with no MANIFEST.json reader, no checked-in expectations file, and a fresh-Runtime-per-test cost that re-evals the 5,207-line testharness.js every test.

**Directories that do not run, or run un-scored:** CSP is effectively 0 (greenfield in the live lane; the only `content-security-policy` refs are legacy Servo `shared/` crates, not wired in); websockets/ and h3 exist in netfetcher but are not wired into the runner; XHTML/.xht files are skipped (`ports/serval-wpt/src/main.rs:587-596`); iframe/second-realm tests are unrunnable, walling off chunks of fetch/ and most of html/; fetch/ runs only behind an off-by-default feature plus a manual hosts-file edit.

## 2. The five biggest conformance levers

Ranked by scoreboard impact, harness-gating first. Two candidate levers were re-measured and **demoted**: gradient color-interpolation is only ~20 of 226 css-images fails (not "the bulk") and is partly GPU-blocked by vello's R9 limitation; right/multiple floats is only ~9 of 57 floats fails (the dominant buckets are BFC-wrap 18, table 7, margin 6). And **webgl-essl is already the live production shader path** (`components/webgl-essl` via `webgl-wgpu` shader lowering) so the 182/235 webgl number already includes it; "swap it in" is not a lever.

1. **MANIFEST.json reader (HARNESS, large).** No `MANIFEST` reader exists in the runner; variant/`?query` expansion, `.any.js`->multi-global enumeration, per-test timeouts, expected-reference resolution and fuzzy metadata are reconstructed heuristically (`main.rs:174`, `:211`, `:719`). No direct pass delta, but it determines what is runnable and scored everywhere. Biggest single lever. *(Plan: WPT harness exactness.)*
2. **object-fit / object-position (ENGINE/CSS, medium).** The real css-images lever, named by none of the candidates: **123 of 226** css-images fails, and replaced-element fit/position is reused by `<img>/<video>/<canvas>` sizing well beyond css-images. Unimplemented (`components/serval-layout/construct.rs:113` is only a comment). ~100 reftests plus spillover. *(Plan: CSS conformance, extended.)*
3. **Snapshot-clone Runtime pool (HARNESS, medium).** The bench probe proves the dominant per-test cost is the testharness.js eval, that naive reuse leaks the `tests` singleton, and prescribes a post-harness-eval `GcAgent::clone` snapshot per test (`harness.rs:393-414`). Prescribed, not built. The throughput substrate for whole-corpus runs, which gates every measurement lever. *(Plan: WPT harness exactness.)*
4. **Per-tag HTML interface hierarchy + interface table (ENGINE/DOM, medium mechanism / incremental payload).** `createElement('button')` returns a generic HTMLElement; only HTMLCanvasElement of ~100 interfaces is wired, and the whole DOM surface is built from a ~900-line JS bootstrap string (`components/script-runtime-api/dom.rs:1066`, `:1989-2015`). Build a declarative interface table to hang the ~100 prototypes + reflected IDL attributes on the existing chain. Honest hedge: adding HTMLElement alone did not fix the dom/nodes count-tail, so this is breadth across many small tests, not one jump; prerequisite for moving custom-elements off 3/2807. *(Plan: HTML interface table.)*
5. **Re-run dom/+fetch corpora, publish aggregates, add a checked-in expectations/regression guard (HARNESS, small-to-medium).** Levers were being sized against numbers that no longer exist. Re-scoring is cheap and restores prioritization. Fold in the Phase-4 expectations file: the difference between a measurement tool and a CI guardrail. Depends on lever 3 to run routinely. *(Plan: WPT harness exactness.)*

## 3. Five best next moves in meerkat

Breadth-first. render.rs is 2001 LOC, input.rs 2405, plus main.rs/content.rs/window_view.rs/card.rs all over Mere's enforced 600-LOC ceiling.

1. **Split render.rs and input.rs under the ceiling.** Factor the ~1700-line `render()` method (`render.rs:299-1999`) into staged passes. Prerequisite for everything else; every move below edits `render()`.
2. **Per-surface scenes.** Generalize `Activation` (one scene/packet/band per member, `constellation.rs:56-133`) to a per-`(member, viewport)` set. The code flags the wart: the focus card is suppressed when the focused node is an open tile because both would drive one content actor at different sizes (last-writer thrash), `render.rs:1093-1101`. Prerequisite for clean multi-window-same-graph.
3. **Finish multi-window fan-out (MW3 5/6).** Structure exists but live chrome writes, a11y, and leaf chrome do not reach secondaries: sync/comms target the primary runner (`app_handler.rs:324-349`), a secondary has no AccessKit bridge and is a full-chrome duplicate (`app_handler.rs:826-848`). *(Folds into the multi_window / tearout plans.)*
4. **HTML-lane link + find parity (render ladder Phase 5).** `link_at` walks an `Activation.links` Vec "empty until Phase 5 lane parity" (`constellation.rs:643-657`); the lane rasterizes one capped texture until then (`render.rs:1286-1287`). Until this lands, click-a-link and find-in-page only fully work on one of the two render lanes. *(Folds into the render ladder plan.)*
5. **Advance the scripted/extraction lane (2026-06-23 plan).** Add a `pump()` before scripted extract so timer/promise content is captured, dispatch keyboard events (needs a serval focus model, flagged "not thin after all"), and give the crawl actor a real frontier (depth, politeness, robots).

> Spun out, with §4, into the mere-side meerkat render-path + GUI-perf plan.

## 4. Five best GUI efficiency / performance improvements

The transform-motion relayout fear is retired (serval spike 2026-06-01, restyle stays RepaintOnly), so the budget is scene rebuilds, allocations, and redraw cadence, not layout. Wire per-pane timers first; the C0 profile harness exists (`render.rs:1988-1993`) but lacks per-pane granularity.

1. **Dirty-gate the redraw loop.** `user_event` ends with an unconditional `request_redraw()` on every actor wake (`app_handler.rs:441-446`), so any fetch/sync/comms/physics poke repaints the whole window. The `card_changed`/`graph_changed`/`comms_changed` flags exist right above it; gate on them and let the orrery self-sustain via `orrery_redraw`. Largest idle win; everything below rides this treadmill.
2. **Cache the orrery scene + memoize node state/shape.** `render()` calls `node_states()` and `node_shapes()` unconditionally every frame (`render.rs:573-578`; `node_states` runs a third time at `:852`), each allocating a fresh HashMap; `Orrery::frame()` rebuilds the arrangement and reprojects all positions every call even when settled. Add a settled-scene cache keyed on generation/camera.
3. **Stop re-encoding favicons every frame.** The `orrery_cards` closure calls `favicon_data_uri` (PNG encode + base64) for every visible node every frame (`render.rs:699-701`), plus per-card label/color/hull allocations, all paid before the PartialEq diff gate. Cache per `(node, favicon-version)` like `snapshot_data_uris` already does (`render.rs:1632`).
4. **Per-surface scenes (same as dev move 2).** Removes the per-frame content re-emit at conflicting viewport sizes and the suppression workaround.
5. **Cache throwaway overlay textures + batch scrying flushes.** While find is open, two 1x1 overlay scenes are rasterized fresh every frame (`render.rs:1566-1588`) unlike the cached decoration textures; scrying `drive()` issues a separate encoder+submit for a 1x1 cache-flush per live tile per redraw (`scrying_host.rs:533-563`). On the netrender side, external-texture interleaving re-renders the whole scene tail into a full-viewport scratch per boundary (`repos/netrender/netrender/src/renderer/mod.rs:1449-1481`); prefer the topmost-overlay path where surfaces allow.

## 5. Five best capability sidequests

Each rides on something already built and tested; the work is wiring.

1. **Activate XPath via `Document.evaluate()`.** `components/xpath` is a complete, test-covered XPath 1.0 engine (3,077 LOC, all axes, ~30 functions) with zero consumers. One binding over its generic trait lights it up for scripting, crawling, and devtools selectors at once.
2. **Wire serval-extract into the Inspector + crawler frontier.** `components/serval-extract` (650 LOC, single dep `layout_dom_api`, zero render stack) already produces title/outline/links/headings/metadata/reader-mode text. The Inspector prints "no EngineDocument diagnostics" for serval pages; the producer exists, only the consumer slice is open. Feeds devtools, a structure-regression oracle, and the eidetic/RAG sink.
3. **Live `<canvas>` WebGL via the external-texture element.** webgl-wgpu (same-device, runs the real Khronos suite) and the `<external-texture>` element view (done end-to-end across four crates, `components/xilem-serval/src/tags.rs:74-82`) are two finished halves never joined. Bind the canvas texture key to the element; no renderer or GL backend to build.
4. **Register graft(Servo) and weld(CEF) as inker SurfaceEngines.** The multiplexer is seated: traits + registry + engine-id vocabulary + one live reference engine (`repos/mere/crates/inker/src/surface_engine.rs:247-358`). wgpu-graft and wgpu-weld already produce importable textures. *(Folds into the inker engine-picker plan, Phase 5; its Phase 0 routing fold-in already shipped, but meerkat's scry pool still binds the producer concretely, so registry producer-construction is the Phase-5 companion.)*
5. **Stand up serval CI + a dependency-cone witness guard.** There is no CI in the serval repo, so the witness boundaries that make the profile ladder real (serval-extract's render-free cone) can silently regress. Cheap, and it protects every sidequest above.

> Sidequests 1, 2, 3, 5 spun out into the capability-activation plan; 4 is tracked in the inker engine-picker plan.

## 6. The wasm question and async

**Viable, deliberately tiered.** `serval-layout` already compiles for wasm32 (P1 done 2026-06-06; the real blockers were jemalloc, uuid randomness, and a few native spots, resolved with cfg gates; the feared ipc-channel/tokio compiled unchanged). Concrete blockers:

- **Nova is wasm32-incompatible, not structurally native-only, and the wasm64 lane now builds** *(corrected and implemented 2026-06-24).* `Value` is 8 bytes on every target (7-byte small payload + 1-byte tag; heap variants are `NonZeroU32`), so `value.rs:398`'s `assert!(size_of::<Value>() == size_of::<usize>())` reduces to `8 == size_of::<usize>()`: false on wasm32 (usize=4), **true on wasm64/memory64** (usize=8). The usize-typed const-overflows (`MAX_UTF16_LENGTH = (1usize << 53) - 1` at `string/data.rs:66`, `2usize.pow(53)` at `data_block.rs:322`) dissolve the same way. Memory64 is **default-on in Chrome/Edge 133 (2025-02-04) and Firefox 134 (2025-01-07)** and absent in Safari/WebKit. Serval now builds a Nova/wasm64 worker and a Boa/wasm32 fallback, selects by validating a minimal Memory64 module, and retries Boa after Nova instantiation failure. USDT/getrandom/single-worker ArrayBuffer blockers are implemented; browser CI and measurement remain the experimental-lane gate. See `2026-06-24_nova_memory64_browser_lane_plan.md`.
- **The renderer is not a structural wasm blocker** *(corrected 2026-06-24; the cited netrender checklist was stale).* That checklist (`wasm-portability-checklist.md`) describes the abandoned `wgpu-backend-0.68-minimal` WebRender/GL branch. On current `main` the WebRender/GL code is deleted (vello is the sole rasterizer, `README.md:7-15`), netrender library src has zero threading and zero GL, boot is async-first with the `pollster::block_on` wrappers `cfg`-gated off wasm32 (`netrender_device/src/core.rs:80-155`), and the device is embedder-supplied (`init.rs:59-64`). Remaining work is ordinary porting: add the wgpu `webgpu`/`webgl` feature (the manifest selects only native backends today) and drive `boot_async` from a browser executor. No wasm32 compile has actually been run, so "compiles clean" is unproven, but the feared structural blockers are confirmed absent.
- **Threading / SharedArrayBuffer** still needs nightly + build-std (for threaded std) + COOP/COEP for SharedArrayBuffer; an external constraint, best answered with Web Workers + postMessage over the actor boundary, not a ported thread pool. Extension nuance: a Chrome MV3 extension can get cross-origin isolation via the `cross_origin_embedder_policy`/`cross_origin_opener_policy` *manifest* keys for extension-owned documents (not the MV3 service worker, which needs an offscreen document); Firefox extensions have no such path yet (bug 1673477).

Near-term viable target: a DOM-only / structured-HTML / smolweb reader PWA (layout is wasm-green, netfetcher binds browser fetch). A Boa-scripted tier is provisioned but is the next milestone, not v1.

**Async is further along than the docs suggest, and dual-engine-tested.** serval models the WHATWG HTML event loop in `script-runtime-api` (Layer 1) on engine-neutral VM primitives, not in any engine: microtask checkpoint, timer task source over a virtual clock, capture/target/bubble dispatch, and a deferred-async fetch seam (`new_host_promise`/`settle_host_promise`) so a native callback mints a Promise and resolves it out-of-band. Boa's Job queue and Nova's async map onto the same shape via the trait; tested on both (`microtasks_on_boa/nova`, `event_loop_on_boa/nova`). Off-thread fetch integrates via the actor model: the engine is single-threaded and `!Send`, the host runs netfetcher off-thread on tokio and wakes the loop, which meerkat already does via `EventLoopProxy` (`repos/mere/crates/meerkat/src/fetch.rs:145-195`).

**Honest gaps are granularity, not architecture:** the loop is cooperative (delays order tasks, they do not truly wait), microtask checkpoints are coarse (around timer batches, not per-task), Boa has no preemption (so `set_deadline` is not a trust boundary; hostile JS needs an externally-killable worker), and ReadableStream is buffered (no BYOB). The event-loop shape is correct; what is missing is concurrency granularity and task-source breadth.

**Concrete closers (added 2026-06-24, from the gterzian/formal-web harvest, `2026-06-24_formal_web_lessons.md`):** the two named gaps have ready reference implementations against the same JS engine (Boa) and same spec. (a) **BYOB** is a bounded port of formal-web's `readablebytestreamcontroller.rs` + `readablestreambyobreader.rs` onto `fetch.rs:842`, with `byob-debug.html` as a ready conformance test — spun out to `2026-06-24_byob_streams_plan.md`. (b) **The coarse microtask checkpoint** is the fine-grained-model fix: move `pump_microtasks` inside the timer loop under the existing `Budget`/`pump` contract, the exact granularity Terzian's FG model caught in Servo — spun out to `2026-06-24_event_loop_rigor_plan.md`, which also carries optional TLA+ trace validation of the scheduler.

**Recommended direction:** stabilize the landed dual-worker lane in Chrome/Firefox/WebKit CI; measure Nova/wasm64 bundle size, startup, and memory before promotion; keep Boa/wasm32 as the portable fallback; finish netrender's wasm port separately; and keep worker termination as the hostile-script trust boundary. Layout/scene transport follows the scripted-DOM milestone rather than entering this baseline. *(Authority: `2026-06-24_nova_memory64_browser_lane_plan.md`.)*

## 7. Next directions for xilem_serval

It is the third xilem_core backend (beside Masonry and xilem_web): "xilem_web, but native, with serval as the engine." It diffs a Xilem view tree into the shared `ScriptedDom` and does no layout/paint/a11y itself. Well past probe stage (Stages 0-7 done, ~6,801 LOC, 54 tests). The clean win: it reuses xilem_core's message cycle unforked, with a uniform `ServalElement = NodeId` element (no AnyNode/downcast).

Ranked directions:

1. **Custom-layout element kind (Mechanism A).** The `<external-texture>` view is done end-to-end but is the only non-flow element, an output-only leaf with no children and no input. The orrery-as-element payoff needs an element whose DOM children are positioned by host/gyre transforms instead of CSS flow, plus a position-only incremental layout path. Designed (mere unified-document-host plan 2026-06-17, spun out to `orrery_custom_layout_element_plan`) but deferred; the host-side `transform:translate` interim holds the visible behavior, so it is the biggest ceiling, not urgent.
2. **Deepen and relocate a11y onto `ServalLaneView`.** `accesskit_tree()` is a live DOM->AccessKit mapping but maps only a handful of tags, folds text into the owner's label, and does not read the ARIA attributes the controls already stamp. Moving it engine-side makes it reusable across every consumer. Cheap relative to payoff.
3. **Cross-path event-model conformance guard.** Two capture->target->bubble dispatchers exist (JS in dom.rs, native in runner.rs `phase_ordered_paths`); they share a `Propagation` cell but are separate code over different trees. Pin the contract with a standing test over the long tail (MouseEvent subclasses, composedPath/shadow retargeting, passive scroll-blocking).
4. **Finish text-editing depth.** Click-to-place + soft-wrap ArrowUp/Down + sticky goal column over the existing parley seam (controls.rs Tier 1 is hard `\n` lines only).
5. **Evaluate a fine-grained update path.** `ServalAppRunner` reruns `logic(&state)` and diffs the whole tree per dispatch (`runner.rs:150-176`). Not yet a measured blocker (binding perf is an explicit non-goal), but a large chrome will make whole-tree-rebuild-per-dispatch hot; a signals layer would be the better impedance match.

> Folded into the element-view + scripted-tier plan as a forward "xilem_serval directions" section; item 1 is tracked in the mere orrery-custom-layout-element plan.

## 8. A vision of serval's architecture

serval is not "a browser engine" in the Servo sense. It is a deliberately decomposed web platform: planes bolted together by narrow engine-neutral seams, with capability dialed by a profile ladder and the web-engine chosen by a multiplexer.

### The planes and their seams

- *Layout/style/paint:* serval-layout (~21k LOC) drives Stylo for the cascade and implements Taffy's traits against its own box tree (not Servo layout; the Stylo firewall holds). It emits an engine-neutral `ServalPaintList` over the closed `PaintCmd` vocabulary in `paint_list_api` (the crate with no netrender dependency). `paint_list_render` lowers to a netrender Scene. netrender (vello-0.9, ~11.9k LOC) is the rasterization leaf and knows nothing about CSS.
- *Network:* netfetcher (WHATWG http(s) Fetch: CORS, RFC6265bis cookies, RFC9111 cache, HSTS, SRI, h1/h2/h3) + errand (smolweb transport, 7 schemes, no http), behind one loader actor routing by scheme. CSP is the one advertised-but-absent capability.
- *Scripting:* the DOM binds to JS not via Web-IDL codegen but a native-sink + JS-bootstrap pattern over the `ScriptEngine` trait; reflectors carry an opaque `u64 NodeId` and DOM data lives in a NodeId-keyed arena (`ScriptedDom`) shared with the native host lane. Above it, `script-runtime-api` models the WHATWG event loop on engine-neutral primitives.
- *Host UI:* xilem_serval diffs into the same `ScriptedDom`, so host chrome and web documents render through one engine, one hit-test, one a11y tree. meerkat already consolidates chrome + all panes into one runner over one DOM.

**The two dials.** The profile ladder (static -> interactive -> scripted -> fullweb) scales capability down as well as up, enforced by witnessed dependency cones (serval-extract proves a tier below paint; layout-on-wasm proves a tier below scripting). The SurfaceEngine multiplexer (mere/inker) scales sideways across web engines via the `compose_external_texture` / `<external-texture>` seam: scry plugged today, graft(Servo)/weld(CEF) exist as standalone producers not yet registered. The vision is code-seated; operationally only one engine is fully plugged, and even scry currently bypasses the registry in meerkat's shipped pool.

**Where it is converging: Blitz-shaped and modular**, with three things Blitz lacks: a multi-engine multiplexer, a profile ladder enforced by dependency-cone witnesses, and host-UI through the same engine. Two assets make it more than a refactor of someone else's engine: webgl-essl (from-scratch pure-Rust ESSL->SPIR-V->WGSL compiler, ~6,500 LOC, 487 tests, already live in production) and serval-extract's render-free lane.

### The three load-bearing bets

1. **`paint_list_api` as the universal display-list contract.** Everything downstream rests on serval producing `PaintCmd` and netrender being one consumer. Current cost: the lowering layer under-delivers serval's real fidelity (`paint_list_render` `ScenePathStroke` is `{color,width}` so cap/join/dash drop; double/groove borders fall to square; several blend modes fall to Normal). Pure translation work, tracked as the netrender lowering tail in the CSS conformance plan.
2. **One `NodeId` arena as the single source of truth for both the JS DOM and the native host tree, behind the `ScriptEngine` seam.** Makes "one DOM two consumers" true and the Nova-native/Boa-wasm split a backend choice. Standing risk: dual-dispatcher drift, policed only by a shared contract + conformance test (hence §7 item 3).
3. **The `compose_external_texture` seam as the multi-engine boundary.** One mechanism behind scry/graft/weld, iframe and WebGL-canvas handoff, and orrery-as-texture. Its biggest payoff and central 60Hz hazard both live here: netrender's tile cache saves CPU scene-rebuild but not GPU re-encode, so the large-static-surface story depends on finishing the compositor-handoff path (D3, serval-side adapter) so the OS applies transform/clip/opacity per surface. netrender's half is done; the bet is unrealized until serval lands the consumer half.

---

## Spun-out plans (this audit's index)

New, serval (this directory):

- `2026-06-24_wpt_harness_exactness_plan.md` — levers 1, 3, 5 (MANIFEST.json reader, snapshot-clone Runtime pool, corpora re-score + checked-in expectations/regression guard).
- `2026-06-24_html_interface_table_plan.md` — lever 4 (declarative per-tag HTML interface table + custom-elements unblock).
- `2026-06-24_capability_activation_plan.md` — §5 sidequests: XPath `Document.evaluate`, serval-extract -> Inspector + crawler, live WebGL `<canvas>` via external-texture, serval CI + witness guard.

New, mere:

- `repos/mere/design_docs/mere_docs/implementation_strategy/2026-06-24_meerkat_render_perf_plan.md` — §3 dev moves + §4 GUI perf.

Extended (existing plans carry this audit's deltas):

- `2026-05-31_css_rendering_conformance_plan.md` — baselines corrected; object-fit/object-position added as the top engine lever; netrender lowering tail tracked.
- `2026-06-06_wasm_enablement_and_crate_rename_plan.md` — §6 forward direction + async-granularity sub-thread.
- `2026-06-16_element_view_and_scripted_tier_plan.md` — §7 xilem_serval directions.
- `repos/mere/design_docs/inker_docs/implementation_strategy/2026-06-15_engine_picker_and_pluggability_plan.md` — sidequest 4 (graft/weld engine registration, Phase 0).
