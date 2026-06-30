# serval-native WPT runner

Status: **superseded/in progress.** Original status was 2026-05-28. Phase 1 (crash-smoke), phase 2
(reftest pixel compare, with linked-resource loading + fuzzy + ref
chains), and phase 3 (testharness.js results, Boa) all built. Phase 4
(expectations) scoped below. Updated 2026-06-30: discovery is now
MANIFEST.json-backed by default, and checked Boa expectation baselines for
full `dom`, focused `dom/abort`, focused `dom/nodes`, and
`html/webappapis/timers` are enforced by a local/CI guard; see
`2026-06-24_wpt_harness_exactness_plan.md`.

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

## Discovery (historical: convention-based; superseded by MANIFEST.json)

Updated 2026-06-29: normal runner commands now discover from
`tests/wpt/meta/MANIFEST.json`; the convention-based walk below remains only as
the `--walk-discovery` diagnostic fallback.

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

   **XHTML (2026-05-31).** Reftests route `.xht`/`.xhtml`/`.xml` (by extension,
   `is_xml_path`) through `StaticDocument::parse_xml` (xml5ever over the shared
   sink) instead of skipping/mis-parsing them; the whole CSS2 `.xht` corpus now
   renders crash-free (normal-flow: 0 errored of 1045) and is scored against its
   ref. Passing those waits on the systematic reftest diff — see the
   [CSS rendering conformance plan](./2026-05-31_css_rendering_conformance_plan.md).
   (testharness `.xhtml` stays skipped — those are XML **+ JS**, a separate axis.)
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
   errors to skipped).

   **Reflected IDL attributes + namespaces + traversal (done 2026-05-29),
   the biggest single lift.** Chosen by a fan-out gap analysis across six
   subsets (a Workflow: an analyzer per failure-log → a synthesis ranking
   levers by subtests-per-cost). The dominant lever was the **reflected
   IDL attribute layer**: the WPT reflection harness checks
   `typeof element[idlName]` for every (element, attribute) pair, and the
   global attributes (`title`/`lang`/`hidden`/`dir`/`tabIndex`/…) are tested
   on *every* element, so present getters of the right kind unlock tens of
   thousands of subtests. Shipped as one increment on the Element/Document
   prototype surface:
   - **Reflected attributes** (Lever 1): a table-driven installer on
     `Element.prototype` for DOMString, boolean, approximate-enumerated
     (lowercased pass-through), and long kinds, all over the existing
     `get`/`set`/`has`/`toggle`/`removeAttribute` sinks. Table built from
     the WPT metadata, conflict-free (idlNames with >1 kind across
     interfaces dropped, since there is one `Element.prototype`). URL /
     tokenlist / double kinds deferred (need URL parsing / exotic objects).
   - **Namespaces** (Lever 2): `localName` / `namespaceURI` / `prefix`
     getters, namespace-gated `tagName` (upper-case only in XHTML),
     `createElementNS` — all from the `QualName` already in the arena.
   - **TreeWalker / NodeIterator / NodeFilter** (Lever 3): pure JS over
     the child/sibling sinks, with the spec filter + traversal algorithms.
   - **Document accessors** (Lever 10): `title` (whitespace-collapsed
     getter + `<title>`-creating setter), `body` setter, `dir`,
     `compatMode`, `readyState`.

   Effect, every measured suite up: **html/dom 4936 → 35366** subtests
   (the reflection volume), **dom/nodes 95 → 832**, **dom/lists 1 → 100**,
   **dom/traversal 1 → 32** (TreeWalker recovered the ERROR files, 7
   all-pass). ~31k subtests gained in one increment.

   **Live collections (done 2026-05-29), Lane C item 1.** HTMLCollection /
   NodeList as legacy-platform exotic objects, via a JS `Proxy` in the
   bootstrap (route b) rather than a new per-backend engine primitive
   (route a) — after verifying both Nova and Boa support the
   `get`/`has`/`ownKeys`/`getOwnPropertyDescriptor` traps the exotic needs
   (`proxy_capability` test). `getElementsByTagName` + `children` return
   live HTMLCollections (length / item / namedItem / indexed + named access
   / `Symbol.iterator`, no `forEach`/`values`); `childNodes` a live
   NodeList, `querySelectorAll` a static one (both with
   `forEach`/`entries`/`keys`/`values`). `getOwnPropertyNames` yields
   indices then deduped non-empty id/name in tree order. So the route the
   earlier note called "needs a new engine primitive" turned out to be a
   pure-JS bootstrap once Proxy was confirmed. Effect: **dom/collections
   3 → 20** (1 all-pass); dom/nodes 832 → 834 (no regression from rewiring
   childNodes/querySelectorAll); html/dom held at 35366.

   **DOMTokenList + dataset (done 2026-05-29), same Proxy route.** With the
   exotic route proven, the cluster the earlier note deferred became cheap.
   `DOMTokenList` is now a real branded iterable (`[object DOMTokenList]`,
   `values`/`keys`/`entries`/`forEach`/`Symbol.iterator`, `.value`, `replace`,
   `supports`, indexed access via a Proxy over a prototype-backed instance);
   `classList` and the `relList` tokenlist reflected kind both route through
   it. `dataset` is a `DOMStringMap` named-property exotic (camelCase
   to/from `data-kebab`, get/set/has/delete/ownKeys), backed by a new
   `__attributeNames` sink. Effect: **dom/lists 100 → 115** (1 all-pass),
   **dom/collections 20 → 25** (2 all-pass), **html/dom 35366 → 35399**
   (11 all-pass), no regressions.

   **DOMImplementation + multi-document (done 2026-05-29).** A fresh
   dom/nodes failure tally showed the dominant lever had shifted to
   `document.implementation`: `hasFeature` (118 subtests) plus the hundreds
   of "createElement(NS) in {HTML,XHTML,XML} document" tests, all failing
   with "cannot convert null/undefined" because the created document was
   undefined. `ScriptedDom` gained `create_document` / `create_comment`
   (detached nodes in the same arena, NodeIds stay unique); the document
   query sinks (getElementById / elementsByTagName / documentElement /
   body / head) now take a **scope ref** so a created document queries its
   own subtree; JS got `document.implementation` (hasFeature /
   createHTMLDocument / createDocument / createDocumentType), `createComment`,
   and `getElementsByTagName` + `getElementsByClassName` shared by Element
   and Document (scoped, live). Effect: **dom/nodes 834 → 1003** (+169),
   **html/dom 35399 → 35515** (+116, 12 all-pass), dom/collections held.
   **Still deferred:** `CharacterData`/`Comment` methods, `Node` identity
   (`isEqualNode`/`compareDocumentPosition`), `DocumentFragment`,
   `cloneNode`, `DOMParser`, URL reflected kind, per-tag HTML interfaces.
4. **Expectations.** A checked-in expected-results file so known
   failures are tolerated and regressions surface (the WPT metadata
   model, serval-shaped). First slice landed 2026-06-29:
   `ports/serval-wpt/expectations/testharness/dom_abort_boa.json`
   checked by `support/wpt/check-testharness-baselines.ps1`.

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
