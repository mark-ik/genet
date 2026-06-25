# Capability activation plan (stranded-capability sidequests)

**Date:** 2026-06-24
**Status:** plan. Spun out of the grand audit (`2026-06-24_grand_audit.md` §5).
**Thesis:** several substantial, tested capabilities exist with no live consumer. Each activation is wiring over finished engineering, disproportionately cheap relative to payoff. This plan groups the four serval-side ones; the fifth sidequest (graft/weld engine registration) is tracked in the inker engine-picker plan (`repos/mere/design_docs/inker_docs/implementation_strategy/2026-06-15_engine_picker_and_pluggability_plan.md`, Phase 0).

## The sidequests (each independent; do in any order)

### C-XPath — Activate `Document.evaluate()`

- Rides on: `components/xpath`, a complete test-covered XPath 1.0 engine (3,077 LOC, all axes, ~30 CoreFunctions) over a generic `Dom`/`Node` trait (`ast.rs:139-201`). Zero consumers today (only its own Cargo.toml references it).
- Work: bind `Document.evaluate()` (and optionally `document.createExpression`) in `script-runtime-api` over the scripted DOM's tree-walking surface.
- **Done when** `document.evaluate(expr, node, ...)` returns an XPathResult driven by the existing engine, exercised by a dom/xpath subset and reused by the crawler/devtools selector path.

### C-Extract — Wire serval-extract into the Inspector + crawler frontier

- Rides on: `components/serval-extract` (650 LOC, single dep `layout_dom_api`, zero render stack) already produces `PageExtract` (title/outline/links/headings/metadata/reader-mode `main_text`) over `LayoutDom`. The Inspector currently prints "HTML rendered through Serval; no EngineDocument diagnostics" for serval pages (`2026-06-13_content_inspection_scope.md:8-46`); meerkat already drains link/metadata contributions off the render path via a crawl actor.
- Work: route `PageExtract` into mere's Inspector read-model; stand up a link-graph crawl frontier over the same render-free `StaticDocument` lane (depth, fan-out, politeness, robots).
- **Done when** the Inspector shows real extraction for serval pages, and the crawler walks a frontier over the render-free lane feeding `GraphContribution`s (and the eidetic corpus). One `extract()` serves devtools, a structure-regression oracle, and the RAG ingest sink.

### C-Canvas — Live WebGL `<canvas>` via the external-texture element

- Rides on: two finished halves never joined. `webgl-wgpu` (6,678 LOC) is a same-device wgpu-backed WebGL state machine that runs the real Khronos conformance suite through `serval-wpt` (`ports/serval-wpt/src/webgl_conformance.rs`) and emits a `WebGlCanvasTexture` on the shared device; webgl-essl is already the live shader path. The `<external-texture>` element view is done end-to-end across four crates (DOM `<external-texture>` -> box-tree replaced box -> `DrawExternalTexture` -> host `compose_external_texture`, `components/xilem-serval/src/tags.rs:74-82`).
- Work: bind the WebGL canvas's output texture key to the element-view, so a `<canvas>` composites its GL output through the same engine-neutral seam.
- **Done when** a WebGL `<canvas>` renders live in a serval document via `compose_external_texture`, giving the WebGL conformance work its first in-page consumer.

### C-CI — serval CI + dependency-cone witness guard

- Rides on: the profile-ladder thesis depends on witnessed dependency cones (serval-extract's render-free cone; serval-layout's wasm-green cone). There is no CI in the serval repo, so a witness-boundary violation (e.g. a render dep creeping into the extract lane) goes uncaught.
- Work: a CI job that builds + tests the workspace and asserts the dependency-cone witnesses (extract lane = `layout_dom_api` only; layout lane wasm32-buildable). Pairs with the WPT-harness plan's H3 expectations guard so conformance regressions also fail CI.
- **Done when** CI runs build+test+cone-assertions on push, and a deliberately-introduced witness violation fails it.

## Why grouped, not five micro-plans

Each is small and independent, but they share a frame (activate finished engineering) and a common beneficiary (the diagnosable/extractable/composable platform). One plan keeps them visible without proliferating docs. C-CI is sequenced loosely first since it protects the others and the conformance plans.

## Non-goals

- Building any new engine or backend; every item is a consumer/binding over an existing tested capability.
- graft/weld engine registration (sidequest 4): tracked in the inker engine-picker plan, Phase 0, not here.

## Findings

- 2026-06-24 (grand audit, verified): xpath 3,077 LOC zero consumers; serval-extract 650 LOC single-dep, producer present consumer absent; webgl-wgpu + external-texture element both done and tested but never joined; no serval CI (rated a major foundational gap).

## Progress

- 2026-06-24 — Plan created from the grand audit. No code yet. Items are independent; C-CI first as the cheapest protective foundation.
