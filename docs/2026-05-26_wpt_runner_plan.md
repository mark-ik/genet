# serval-native WPT runner

Status: **in progress (2026-05-26).** Phase 1 (crash-smoke) and phase 2
slice 1 (reftest pixel compare, inline-only, exact match) built; phase 2
refinements + phase 3 scoped below.

## Goal

Run [web-platform-tests](https://github.com/web-platform-tests/wpt)
(the corpus under `tests/wpt`) against serval, **selecting individual
subsets** so a single subsystem can be checked without running the whole
suite. serval-native (cargo + the engine crates), not Servo's removed
mach/python `wptrunner`.

## Subset selection (first-class)

The runner takes a path under the tests root:

```shell
serval-wpt list css/CSS2/floats          # enumerate + classify
serval-wpt run  css/CSS2/floats          # run that subset
serval-wpt run  dom/nodes/Element-classlist.html   # a single file
serval-wpt run css --tests-root tests/wpt          # explicit root
```

Default tests root is `tests/wpt`. A subset is any directory or file
beneath it; the runner walks only that path.

## WPT test types and how serval verifies each

- **crashtest** / load smoke: pass if loading (parse + cascade + layout)
  does not panic. No GPU, no JS. **Phase 1.**
- **reftest**: render the test and its `rel="match"`/`"mismatch"`
  reference, compare pixels. Needs the paint + netrender readback path.
  **Phase 2.**
- **testharness.js**: load `testharness.js`, run the test's assertions,
  collect per-subtest results over the testharness protocol. Needs the
  scripting tier + a results hookup. **Phase 3.**
- **manual** / **wdspec**: skipped.

## Discovery (convention-based, no MANIFEST.json yet)

Servo's `wptrunner` consumed a generated `MANIFEST.json`; that generator
was python and is gone. Phase 1 classifies by convention instead:

- Skip references (`*-ref.*`, `ref-*`, `*.ref.*`, files under
  `.../reference/`) and `*-manual.*`.
- **reftest**: HTML contains `<link rel="match">` or `rel="mismatch">`.
- **crashtest**: path under a `crashtests/` dir, or `*-crash.*`.
- **testharness**: references `testharness.js`.
- else: treated as a load test (run through the crash-smoke).

A real `MANIFEST.json` reader can replace this later for exactness
(ref chains, test variants, timeouts).

## Phases

1. **Crash-smoke + subset selection (this).** `list` + `run`; `run`
   loads each runnable test through `serval_static_dom::parse` +
   `serval_layout::render` (with inline `<style>` extracted), wrapped in
   `catch_unwind`. Reports per-test survival + a summary. Finds layout
   panics across real pages, which is the highest-leverage early signal.
   No GPU.
2. **Reftests.** Render test + reference to images (the
   `html_to_pixels_e2e` path: cascade → layout → emit → netrender →
   readback) and pixel-compare, with match/mismatch semantics. GPU,
   booted once per run. **Slice 1 (done):** inline `<style>` only, exact
   compare; tests needing linked CSS / scripts are skipped. First signal
   on `css/CSS2/floats`: 7 passed, 92 failed, 98 skipped of 197.
   **Refinements (next):** linked stylesheets + local images (relative +
   `/`-absolute resolution); fuzzy matching (WPT `fuzzy` metadata) so
   anti-aliasing diffs do not count as failures; ref chains.
3. **testharness.js.** Once the scripting tier runs testharness, capture
   subtest results. Gated on JS execution maturity.
4. **Expectations.** A checked-in expected-results file so known
   failures are tolerated and regressions surface (the WPT metadata
   model, serval-shaped).

## Non-goals (for now)

- The full WPT server (`wpt serve`) and its routing. Phase 1/2 load files
  directly; tests needing the server (cross-origin, dynamic handlers) are
  out until a minimal host exists.
- Parallelism, sharding, retries. Add when the corpus size warrants.

## Crate

`ports/serval-wpt` (bin). Phase 1 deps: `serval-static-dom`,
`serval-layout`, `layout_dom_api`. Directory walking is hand-rolled
(`std::fs`) to avoid a new dependency. Phase 2 adds the paint/netrender
deps.
