# serval-native WPT runner

Status: **in progress (2026-05-28).** Phase 1 (crash-smoke), phase 2
(reftest pixel compare, with linked-resource loading + fuzzy + ref
chains), and phase 3 (testharness.js results, Boa) all built. Phase 4
(expectations) scoped below.

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
3. **testharness.js (done 2026-05-28).** `serval-wpt testharness <subset>`
   runs each testharness test on the host surface and collects per-subtest
   results. For each test it extracts the test's own scripts (inline
   `<script>` + local `<script src>`, skipping `testharness.js` / the
   report hook), loads `testharness.js` on a fresh `Runtime`, runs the
   scripts, drives completion (dispatch `load` + drain the loop), and reads
   results via the bridge (`Runtime::run_testharness`, `script-runtime-api`).
   Engine: **Boa** (Nova's regex engine rejects the harness's surrogate
   sanitizer — see the
   [pluggable-engines plan](./2026-05-26_pluggable_engines_testharness_plan.md)).
   Reports per test (all-pass / with-failures / errored / no-results) and
   an aggregate subtest count. First run on `dom/nodes` (331 files):
   1 all-pass, 219 with-failures, 58 errored, 7 no-results, 46 skipped;
   **30/1858 subtests passed.** The low rate is the expected signal — the
   error lines are a punch-list of missing DOM breadth.

   **Body-DOM parsing (done 2026-05-28).** `Runtime::load_dom` clones the
   test's parsed HTML (any `LayoutDom`) into the scripted document before
   running script, and `document.body` / `documentElement` / `head` resolve.
   So tests querying body elements run instead of erroring at the first
   query. On `dom/nodes` this broadened *execution* — subtests run
   1858 → 2538, hard errors 58 → 54 — without yet lifting the pass count
   (still 30). The honest finding: body-DOM is necessary but not
   sufficient; the now-running subtests hit the *next* wall immediately.
   The failure messages name it precisely: missing `Element`/`Node` methods
   (`toggleAttribute`, `hasAttribute`, `matches`, `querySelector`,
   `classList`), attribute reflection getters (`el.id`, `el.className`), and
   `assert_throws_dom` needing a real `DOMException` with `.code`. Example:
   `dom/nodes/attributes.html` now runs 4/67 (was erroring at 0).

   **Element surface (done 2026-05-28).** The Element method/reflection
   breadth the failures named: a JS `Element`/`Text`/`Document` prototype
   split (`instanceof`, `nodeType`), `hasAttribute`/`removeAttribute`/
   `toggleAttribute`, `id`/`className` reflection, `classList` (a
   `DOMTokenList` over the class attribute), and `querySelector`/
   `querySelectorAll`/`matches` backed by a self-contained selector matcher
   (`selector.rs`: type/`*`/`#id`/`.class`/`[attr]`/`[a=v]`/`[a~=v]` +
   descendant/child combinators; unsupported syntax safely matches
   nothing). Plus `DOMException` (name→code table), `requestAnimationFrame`,
   and a real `textContent` getter (aggregates descendant text). Found and
   fixed in passing: `textContent` was reading only the node's own text;
   now it concatenates descendant text nodes per spec.

   Effect on `dom/nodes` — the number moved on every axis: subtests run
   2538 → **3341**, subtests passed 30 → **61**, errored files 58 → **46**,
   all-pass files 0 → **2**. The arc (harness + results + body-DOM +
   Element surface) is a working conformance loop with a rising number.

   **Node/Element traversal (done 2026-05-29).** The tree-navigation
   surface `dom/nodes` is mostly about: `childNodes` / `firstChild` /
   `lastChild` / `nextSibling` / `previousSibling` / `parentElement`,
   element-filtered `children` / `firstElementChild` /
   `nextElementSibling` / `childElementCount`, `nodeName` / `nodeValue`,
   `hasChildNodes` / `contains`, the mutators `removeChild` /
   `insertBefore` / `replaceChild` (throwing `NotFoundError` when the node
   isn't a child — so `assert_throws_dom` starts passing), and the
   `ChildNode` mixin (`remove` / `before` / `after` / `replaceWith`). Plus
   two correctness fixes: the runner now **skips `.xhtml`** (XML parse mode
   serval's HTML parser doesn't handle — was ~14 spurious syntax errors),
   and `serval-scripted-dom` gained `remove_child` (DOM `removeChild`
   *orphans* a node, keeping it alive + re-insertable, vs `LayoutDomMut::
   remove` which drops the subtree — script holds references to removed
   nodes).

   Effect on `dom/nodes`: all-pass files 2 → **14**, subtests passed
   61 → **95**, errored files 46 → **28** (the xhtml skips moved ~14 false
   errors to skipped). **Next levers:** DOM methods throwing `DOMException`
   on bad input more broadly, `createElementNS` / namespaces, `Comment` /
   `DocumentFragment` node types, `cloneNode`, and missing globals
   (`customElements`).
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
