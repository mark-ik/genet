# Pluggable engines to testharness-level testing

Status: **landed for Boa + Nova (2026-06-02); QuickJS/SpiderMonkey deferred.**
The WPT testharness runner takes `--engine boa|nova` and produces real WPT
numbers on both in one binary (see Sequencing 4). Originally scoped to run
`testharness.js` against genet with the JS engine selectable (Boa, Nova,
QuickJS, optionally SpiderMonkey). Child of the
[script engine plan](./2026-05-20_genet_script_engine_plan.md): that
doc owns the `ScriptEngine` trait, the crate ladder, and the reflector
findings (Appendix A/C). This doc answers one focused question the
parent leaves implicit: *what has to exist for each engine to reach
testharness*, and in what order.

## The framing

The engine is the cheap part (parent plan, "the engine is the cheap
part"). Two facts set the whole shape:

1. **The dominant blocker is shared and engine-independent.** genet's
   live JS today exposes one builtin (`setText`) and one reflector
   (`node`), wired through `thread_local`s
   ([genet-scripted/lib.rs](../components/genet-scripted/lib.rs)).
   `testharness.js` (5,207 lines) needs, at load, a global scope
   (`self`/`globalThis`), `addEventListener`, `postMessage`, and
   `document` for window-scope tests, with defensive fallbacks for
   `setTimeout` and worker scopes. That gap is the
   `script-runtime-api` layer (parent plan, Layer 1): the global, the
   DOM/Event surface, the event loop with timers and microtasks, and a
   results bridge. It is written once against the VM trait and reused by
   every backend.

2. **Per-engine cost is confined to one seam: the reflector.** The
   binding layer needs each DOM node to carry a traced, host-associated
   JS object so callbacks recover the `NodeId` and the DOM↔JS cycle stays
   collectable (parent plan, Appendix A Finding 2: this needs a
   `ScriptEngineLive` extension, not plain `new_function`). How you
   create that object differs per engine. Everything above it does not.

So "make engines pluggable" and "reach testharness" are separable, and
only the reflector is per-engine. Build the binding layer once against
the trait; each backend then needs the VM primitives plus its reflector
mechanism.

## What the binding layer must provide for testharness

The minimum `script-runtime-api` surface to load `testharness.js` and a
window-scope test, assembled from `ScriptEngine` primitives
(`new_function`, `set_global`, `pump_microtasks`) plus `ScriptEngineLive`
reflectors:

- **Global scope.** `self` / `globalThis` resolving to the global;
  `window` aliasing it in window scope; `location` (a stub URL is enough
  to load the harness).
- **EventTarget.** `addEventListener` / `removeEventListener` /
  `dispatchEvent` on the global and on nodes, plus an `Event` /
  `CustomEvent` shape. testharness registers `load`, `message`, and
  `error` listeners at setup.
- **Event loop.** `setTimeout` / `setInterval` / `clearTimeout` and a
  task + microtask drain (the rakers Backburner lesson: both queues
  drain together). testharness has a `fake_set_timeout` fallback, but
  real timer ordering is needed for the bulk of `dom/` and `html/`.
- **postMessage.** Used for worker and cross-document result
  collection; a same-process channel covers the common cases.
- **Document + Node/Element.** Enough of `document`,
  `Node`/`Element`/`CharacterData`, `getElementsByTagName`,
  `getElementById`, attributes, and `textContent` for the harness setup
  (it reads `<meta>` for timeouts) and for the tests under measurement.
  This is the open-ended axis (parent plan calls it binding breadth);
  testharness *loading* needs only a thin slice, while the tests it then
  runs need progressively more.
- **The results bridge.** Read the `tests` / `Test` status objects back
  out at completion and map them to the runner's per-subtest results.
  This is the testharness-specific piece the reftest path has no analog
  for.

The DOM side of this binds against `LayoutDomMut` on `genet-scripted-dom`
(the JS→DOM direction already proven by the `setText` reflector), with
relayout driven by the existing `IncrementalLayout` engine.

## Per-engine cost to reach testharness

All four implement the same `ScriptEngine` + `ScriptEngineLive` traits;
the column is what differs.

| Engine | Status | Reflector mechanism | Cost to testharness |
| --- | --- | --- | --- |
| **Nova** (default) | live on 64-bit targets, including wasm64 | `EmbedderObject` carrying the `NodeId` (in use today; Appendix C: `get_or_create_backing_object`) | The trait and host data are live. The wasm64 lane gates USDT and uses the same reflector contract; wasm32 remains Boa-only. |
| **Boa** | in workspace (fork, native + wasm) | `JsClass` / `NativeObject` with `Trace` | Second implementation; lowest friction (pure Rust, mature class + `HostDefined` story). The conformance oracle: when Nova and Boa disagree on a result, Boa is the language-axis reference (~94% test262 vs Nova ~80%). |
| **QuickJS** | not yet | class-id + opaque pointer; refcount rooting (`JS_DupValue`/`JS_FreeValue`) | Bind through `rquickjs`; conceptually clean, adds a small C build dep and an FFI seam in the impl. wasm-ok. |
| **SpiderMonkey** | excised (was mozjs) | reserved slots / private data; JSAPI rooting | Heaviest: re-admits the build environment the fork removed. Only behind a feature gate, fullweb-only, if a near-spec reference is needed. Structurally absent from the wasm graph (parent plan, "witness, don't gate"). |

## Sequencing

Binding-layer-first, against the trait, so each engine is "add an impl,"
not "add a harness."

Current state (2026-05-26): `script-engine-api` carries the contract
`ScriptEngine` (`new` / `eval` / `value_to_string` / `set_global` /
`set_host_data` / `set_function`) + `ScriptEngineLive` (`make_reflector`
/ `reflector_data`) + the native-callback surface (`NativeFn` trait +
the one-lifetime `CallCx` GAT + `HostData`). `script-engine-nova`
(native, primary) and `script-engine-boa` (pure Rust, wasm + oracle)
both implement all of it, validated by parallel tests: reflector
round-trip, global-reflector reachability, and a `setText`-style native
callback that reaches host state and reads a reflector argument through
real JS execution. `set_global`, host data, and `set_function` landed
2026-05-26.

The native-callback host-state decision is settled: **host-defined
data, both engines, no `thread_local`.** A callback is a zero-sized type
implementing `NativeFn`; each backend registers a monomorphized,
captures-free bare `fn` trampoline (Nova `RegularFn`, Boa
`NativeFunctionPointer`). State reaches it through `CallCx::host_data`
(Nova realm `[[HostDefined]]`, Boa `Context` host data) and the
reflector arguments. Nova's two-lifetime `GcScope` collapses onto the
one-lifetime `CallCx` GAT using `GcScope`'s covariance in its second
lifetime.

1. **Rewire `genet-scripted` onto `script-engine-nova`. (done
   2026-05-26.)** Its inline Nova code (the `setText` builtin + `node`
   reflector + the `thread_local` host-DOM) is replaced by `NovaEngine`
   with `set_function::<SetText>`, `set_global`, and `set_host_data`; the
   host DOM (`Rc<RefCell<ScriptedDom>>`) is the `HostData`. The existing
   `js_mutates_dom_through_reflector` test stays green, so the primitives
   are proven driving real DOM mutation. `genet-scripted` no longer
   names `nova_vm` directly (it is transitive via `script-engine-nova`),
   and the duplicate Nova path is retired.
2. **Build the host surface against the trait** (in progress,
   2026-05-26/27). Two layers, per the
   [web-platform-API shared-middle plan](./2026-05-25_web_platform_api_shared_middle_plan.md)
   (that doc is the *interior* this step's catalogue half fills in):
   - **`script-runtime-api` = the host shell.** `Runtime<E>` over any
     backend, the aggregated `HostState` host-data slot, global aliases
     (`self` / `window`), `console`, the cooperative **event loop**
     (`setTimeout` / `setInterval` / `clear*`, drained by
     `run_event_loop`), and the **global-scope EventTarget** / `Event`.
     These are genuine host concerns with no DOM tree, so they are JS
     bootstraps over `eval` + `set_function`. Validated on Nova and Boa.
   - **The DOM interface catalogue = `web-api` behavior** (grown inside
     `script-runtime-api`'s `dom.rs` for now; extract a `web-api` crate
     when the catalogue's shape justifies it, **without** WebIDL
     codegen, which the `CallCx` work made unnecessary). Behavior is
     native Rust over `LayoutDomMut`, written once and bound through
     `CallCx` (not per-engine): the `CallCx` marshaling surface
     (`arg` / `value_to_string` / `reflector_data` / `make_reflector` /
     `undefined`) collapsed the 2026-05-25 plan's per-engine edges to a
     single neutral binding. **W0a (done):** the construction/mutation
     sinks — `createElement` / `createTextNode` / `appendChild` /
     `setAttribute` / `textContent` setter / `getElementById` — mutate
     the host `ScriptedDom` natively, bound through `CallCx`, validated
     on both backends. The `setText` probe is generalized, not replaced.

   The `document` *global slot* is installed by the shell; the `Document`
   *interface behavior* lives in the catalogue. That split resolves the
   apparent shell-vs-catalogue contradiction (both docs are right about
   their own half).

   **W0b (done 2026-05-27):**
   - **Read primitives** `make_string` / `make_null` on `CallCx` (the
     mirror of `make_reflector`), feeding `getAttribute` / `tagName` /
     `textContent` getter and a real `null` on a miss.
   - **Reflector identity** (`getElementById('x') === getElementById('x')`).
     `CallCx::reflector_for(node)` returns a **canonical** reflector
     (minted once, cached), and `wrapNode` caches wrappers keyed on it.
     The cache is **engine-side** (the neutrality wall): per-engine
     `NodeId → reflector` maps in each engine's host-defined slot — Nova
     `Global`s self-root, Boa `JsValue`s are GC-traced. This is the
     precise residue of "what stays per-engine": identity caching, not
     marshaling. Validated `===` on both backends.

   **W0c (done 2026-05-27):**
   - **Prototype-based dispatch.** `Node` / `Document` prototypes
     (`Document : Node`), methods shared on the prototype not per-object,
     `instanceof` works, `parentNode` walks the real tree. Wrappers are
     `Object.create(Node.prototype)`; the cache keeps identity.
   - **Node-level EventTarget** with real tree propagation: `capture →
     target → bubble` over the `parentNode` chain, `target` /
     `currentTarget`, `stopPropagation`, and `preventDefault` flipping
     the dispatch result. Validated on both backends.

   **W0 is essentially complete.** Remaining DOM breadth is later tiers:
   the `Element` / `Text` split (all wrappers are `Node` today) and W2
   reflection.

   **Microtasks (done 2026-05-27).** `ScriptEngine::pump_microtasks`
   drains the Promise job queue to quiescence — the last engine-trait
   gap (everything else rode `eval` + `set_function` + host data). Nova
   needed a custom `HostHooks` capturing promise/generic/timeout jobs
   into a shared queue (`Job`s are `'static`, so queuing across GC is
   safe; the hooks are leaked per engine for the `&'static` requirement);
   Boa forwards to `Context::run_jobs`. The runtime runs a microtask
   checkpoint via `run_microtasks` and around the timer batch in
   `run_event_loop` (coarse interleaving; per-task checkpoints later).
   Validated on both backends: `.then` continuations, chained drains,
   and a promise resolved inside a timer.

   **postMessage (done 2026-05-27):** async `message` delivery to the
   global over the event loop, plus minimal `location` / `navigator`
   stubs the harness reads at load.
3. **`testharness.js` loads (done 2026-05-27).** The real WPT
   `testharness.js` (5,207 lines) parses and evaluates to completion on
   **both** Nova and Boa, and its API (`test` / `async_test` /
   `promise_test` / `done`) is present. It builds a `WindowTestEnvironment`,
   wires `error` / `unhandledrejection` / `message` handlers, and reaches
   `on_tests_ready` without throwing. The one blocker found and fixed was
   `document.getElementsByTagName` (the harness reads `<meta name=timeout>`
   at startup); added as a tree-walk (count + item sinks, no array-minting
   primitive yet).

   **Results bridge (done 2026-05-28).** `Runtime::run_testharness` loads
   the harness, registers an `add_completion_callback` that forwards each
   subtest to a `__reportResult` native sink (into `HostState::results`),
   evaluates the test, and drives completion by dispatching the window
   `load` event + draining the loop. A real test (a passing and a failing
   `assert_true`) runs end-to-end and the per-subtest pass/fail comes
   back. Two more host needs were found and met along the way: `window`'s
   self-referential `parent`/`top` (testharness walks `while (w != w.parent)`)
   and disabling HTML output via `setup({output:false})` (the output path
   renders a results table through `createElementNS`/`body`, which we don't
   implement — a headless runner reads results programmatically instead).

   **First cross-backend engine-axis delta, concretely.** The bridge
   passes on **Boa** (the conformance oracle). On **Nova** it loads and runs
   the DOM/event/microtask paths fine, but the harness's completion step
   (`sanitize_all_unpaired_surrogates`) compiles a regex with lone-surrogate
   ranges (`[\ud800-\udbff]`) that Nova's regex engine (`regress`) rejects
   ("not a Unicode scalar value"). JS regex is UTF-16; the engine wants
   scalar values. This is an upstream Nova gap, not a binding-layer bug.

   **Surrogate scrub neutralized (done 2026-06-02).** That one regex is
   purely cosmetic — `sanitize_unpaired_surrogates` scrubs lone surrogates
   from test-*name* strings before results cross a UTF-8 transport, which a
   headless in-process runner never does (it reads `test.name`/`status`
   directly). `Runtime::run_testharness` now rewrites that single regex
   literal (unique in the file, verified) to a never-matching char-class
   pattern (`/[^\s\S]/g` — not `/(?!)/`, which uses look-around Nova also
   rejects) before eval, via `harness_regex_compat`. Headless results
   are byte-identical; the harness now *compiles* on a scalar-only regex
   engine. Engine-neutral (no-op on Boa). The Nova results test is
   un-`#[ignore]`d and **passes**.

4. **Run the same harness through Boa *and* Nova (done 2026-06-02).** The
   WPT runner takes `--engine boa|nova` (Boa default); `harness::run_test`
   is generic over `ScriptEngine`, dispatching to a `run_with::<E>` core, so
   the engine is just the monomorphization. The summary line is tagged
   `testharness [engine]:`. Both backends now produce real WPT numbers in
   one binary, making the engine-axis delta observable per the two-axis
   framing.

   **It immediately paid off** — see the divergence map below.
5. **QuickJS / SpiderMonkey** become incremental backend additions when
   wanted, not new harness work.

## Boa↔Nova divergence map (sweep, 2026-06-02)

A both-engines sweep over `dom/traversal` (18), `dom/collections` (9), and
`dom/nodes` (263) with a normalized diff (compare per-file status + subtest
count, ignoring the engine-specific *error-message text*, which is cosmetic
not behavioral). Findings, in order of value:

- **The binding layer is engine-neutral.** `dom/traversal` (0 behavioral
  divergences — the 4 errored files error *identically* on both, a shared
  binding gap), `dom/collections` (identical), `Element-classList` (785/1420
  on both). Where both engines reach a test, they agree.

- **One Nova regex bug cascaded across 82 `dom/nodes` files** — the largest
  delta by far. testharness.js L581 (`assert_throws_*` failure formatting)
  matches `func.toString()` against `^\(\)\s*=>\s*(?:{(.*)}\s*|(.*))$` to
  pretty-print `() => {...}`; Nova's `regress` mis-reads the literal `{` after
  `(?:` as a `{n,m}` quantifier ("repetition operator missing expression").
  It is harness machinery (not the spec behavior under test), so
  `harness_regex_compat` escapes the braces (`\{`/`\}` — exactly equivalent,
  accepted by both). That single shim took Nova `dom/nodes` from ~90 errored
  to **19**, and ~0 → **1590/4500 subtests** (Boa 1646/5325). The underlying
  `regress` bug (literal `{` outside quantifier position should be a literal,
  per Annex B) is still worth upstreaming to Nova.

- **5 Nova-only panics** (`Document-createElement`, `createElementNS`,
  `getElementsByClassName-N` / `-whitespace-class-names`,
  `CharacterData-surrogates`) — Boa runs all five to completion; Nova
  **panics** (a Rust crash, caught by the runner's `catch_unwind`, worse than
  a JS throw). Shared root cause: these tests push **lone surrogates** through
  DOM string methods (`substringData`/`replaceData` splitting `\uD83C`/`\uDF20`
  pairs; invalid-QName `createElement`). The binding's `value_to_string` is
  safe (`to_string_lossy`), so the panic is **inside `nova_vm`'s own string
  machinery** when JS builds a lone-surrogate string — an upstream Nova bug,
  distinct from the binding layer. The highest-severity residual; upstream-fix.

- **Thin tail of binding gaps** (post-shim, ~5 files): missing globals
  `customElements` / `HTMLElement` / `URL` / `frames`, and "Not a callable
  object" for unimplemented APIs. These are *genet-surface* gaps, not engine
  divergences — they'd fail on Boa too once it reaches them; the diff only
  shows them because the engines throw at slightly different points.

Net: divergence collapsed **91 → 10 files** with one equivalence-preserving
harness shim; the genuine residual is **one upstream Nova regex bug** (now
worked around) and **one upstream Nova lone-surrogate string panic** (5 files,
unfixed). The two-axis runner did its job — it turned "Nova conformance is
unmeasured" into a short, located, upstream-actionable list.

## Relationship to existing docs

- [Script engine plan](./2026-05-20_genet_script_engine_plan.md) owns
  the trait, the `script-engine-*` / `script-runtime-api` ladder, and
  the reflector findings. This doc does not restate them; it pins the
  testharness-specific binding slice and the per-engine cost to reach it.
- [JS execution strategy](./2026-05-25_js_execution_strategy.md) covers
  the orthogonal performance axis (interpret vs JIT vs AOT vs weval),
  which does not affect reaching testharness.
- [WPT runner plan](./2026-05-26_wpt_runner_plan.md) phase 3 is the
  consumer: once the binding layer loads testharness on a backend, the
  runner captures subtest results.

## Non-goals

- Performance of the binding layer. Correctness and breadth first;
  the strategy doc handles speed later.
- A complete DOM. The target is "testharness loads and the measured
  tests run to completion," then breadth ratcheted per the parent
  plan's T-scripted-breadth targets.
- The full WPT server. Same boundary as the reftest runner: files load
  directly until a minimal host exists.
