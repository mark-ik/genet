# Capability activation plan (stranded-capability sidequests)

**Date:** 2026-06-24 (status refresh 2026-07-16)
**Status:** active integration plan. Spun out of the grand audit (`2026-06-24_grand_audit.md` §5).
**Thesis:** several substantial, tested capabilities exist with no live consumer. Each activation is wiring over finished engineering, disproportionately cheap relative to payoff. This plan groups the four genet-side ones; the fifth sidequest (graft/weld engine registration) is tracked in the inker engine-picker plan (`repos/mere/design_docs/inker_docs/implementation_strategy/2026-06-15_engine_picker_and_pluggability_plan.md`, Phase 0).

## The sidequests (each independent; do in any order)

### C-XPath — Activate `Document.evaluate()`

- Rides on: `components/xpath`, a complete test-covered XPath 1.0 engine (3,077 LOC, all axes, ~30 CoreFunctions) over a generic `Dom`/`Node` trait (`ast.rs:139-201`). Zero consumers today (only its own Cargo.toml references it).
- Work: bind `Document.evaluate()` (and optionally `document.createExpression`) in `script-runtime-api` over the scripted DOM's tree-walking surface.
- **Done when** `document.evaluate(expr, node, ...)` returns an XPathResult driven by the existing engine, exercised by a dom/xpath subset and reused by the crawler/devtools selector path.

### C-Extract — Wire genet-extract into the Inspector + crawler frontier

- Rides on: `components/genet-extract` (650 LOC, single dep `layout_dom_api`, zero render stack) already produces `PageExtract` (title/outline/links/headings/metadata/reader-mode `main_text`) over `LayoutDom`. The Inspector currently prints "HTML rendered through Genet; no EngineDocument diagnostics" for genet pages (`2026-06-13_content_inspection_scope.md:8-46`); the current host checkout already drains link/metadata contributions off the render path via a crawl actor.
- Work: route `PageExtract` into mere's Inspector read-model; stand up a link-graph crawl frontier over the same render-free `StaticDocument` lane (depth, fan-out, politeness, robots).
- **Done when** the Inspector shows real extraction for genet pages, and the crawler walks a frontier over the render-free lane feeding `GraphContribution`s (and the eidetic corpus). One `extract()` serves devtools, a structure-regression oracle, and the RAG ingest sink.

### C-Canvas — Live WebGL `<canvas>` via the external-texture element

- Rides on: the lower seam is now integrated. `webgl-wgpu` is a same-device
  WGPU-backed WebGL state machine, `webgl-essl` is its live shader frontend,
  and `genet-wpt` now runs the real `gl-clear.html` fixture through the JS
  binding. `ScriptedDocument` accepts a pre-script `WebGlFactory` for both Boa
  and Vano's `nova_vm` backend, and the Genet render frame carries the canvas
  texture key and placement as external-texture metadata.
- Work remaining: construct the factory from Merecat's shared WGPU device,
  register each canvas output in the compositor's texture registry, and feed
  the frame metadata into `compose_external_texture`.
- **Done when** a WebGL `<canvas>` renders live in a Genet document through the
  host's shared-device `compose_external_texture` path. The first Khronos
  fixture is already green; broader categories remain a separate ratchet.

### C-CI — genet CI + dependency-cone witness guard

- Rides on: the profile-ladder thesis depends on witnessed dependency cones (genet-extract's render-free cone; genet-layout's wasm-green cone). There is no CI in the genet repo, so a witness-boundary violation (e.g. a render dep creeping into the extract lane) goes uncaught.
- Work: a CI job that builds + tests the workspace and asserts the dependency-cone witnesses (extract lane = `layout_dom_api` only; layout lane wasm32-buildable). Pairs with the WPT-harness plan's H3 expectations guard so conformance regressions also fail CI.
- **Done when** CI runs build+test+cone-assertions on push, and a deliberately-introduced witness violation fails it.

## Why grouped, not five micro-plans

Each is small and independent, but they share a frame (activate finished engineering) and a common beneficiary (the diagnosable/extractable/composable platform). One plan keeps them visible without proliferating docs. C-CI is sequenced loosely first since it protects the others and the conformance plans.

## Non-goals

- Building any new engine or backend; every item is a consumer/binding over an existing tested capability.
- graft/weld engine registration (sidequest 4): tracked in the inker engine-picker plan, Phase 0, not here.

## Findings

- 2026-07-16 refresh: xpath 3,077 LOC zero consumers; genet-extract 650 LOC
  single-dep, producer present consumer absent; the Genet WebGL factory,
  frame metadata, and first Khronos fixture are live, while host WGPU texture
  registration remains open; no genet CI (rated a major foundational gap).

## Progress

- 2026-06-24 — Plan created from the grand audit.
- 2026-07-16 — C-Canvas lower-layer integration landed: pre-script factory,
  external-texture frame metadata, and green `gl-clear` harness receipt on
  Boa and Vano-backed `nova_vm` paths. Host registry/composition is the next
  boundary.
