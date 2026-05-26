# serval-native WPT runner

Status: **in progress (2026-05-26).** Phase 1 (crash-smoke) and phase 2
(reftest pixel compare, with linked-resource loading + fuzzy + ref
chains) built; phase 3 scoped below.

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
   booted once per run. **Done:** inline `<style>` + linked
   `<link rel="stylesheet">` + local images (resolved relative to the
   test dir, `/`-absolute to the tests root; remote `http(s)://` renders
   as missing, `data:` decodes inline); fuzzy matching from
   `<meta name="fuzzy">` (max per-channel diff + max differing pixels),
   exact when absent; `match` ref chains followed to the final reference
   (capped); only `<script>` tests are skipped (no JS yet). Verified: the
   linked-CSS loader fires (`css/css-backgrounds/box-shadow` reftests now
   render + compare, not skip). `css/CSS2/floats` is unchanged (100%
   inline-style, so the refinements are correct no-ops there: 7/92/98).
   **Honest caveat:** linked-CSS is confirmed firing; the fuzzy + ref-chain
   paths run without error but their pass-changing effect is not yet
   demonstrated on a specific subset. **Still out:** remote resources (the
   WPT server), `mismatch` chains.
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
