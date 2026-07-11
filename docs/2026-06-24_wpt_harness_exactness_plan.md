# WPT harness exactness + throughput plan

**Date:** 2026-06-24
**Status:** in progress — H1 is wired into the normal runner path (manifest-backed discovery by default; legacy walk demoted to `--walk-discovery`), **H2a (Nova snapshot clone through the harness)**, H2b (cross-engine compare), **H3 (checked-in testharness expectations + local/CI guard)**, and **H5 (test262 runner + hang-safe full-corpus run)** landed. The remaining H2 question is no longer "can we wire snapshots?", but "is the Nova lane stable enough on broad release-mode corpora for the throughput win to matter?" H4 is partial: per-test reason metadata landed, but the full opt-in policy / per-subtest metadata layer remains open. Spun out of the grand audit (`2026-06-24_grand_audit.md` §2, levers 1/3/5); continues and supersedes the discovery/expectations portions of the WPT runner plan (`2026-05-26_wpt_runner_plan.md`).
**Thesis:** the binding constraint on serval's WPT scoreboard is the harness, not the engine. What runs and how much runs gates the value of every engine fix. This plan closes the three harness levers in dependency order: exactness (what runs), throughput (how much runs), then a tracked scoreboard + regression guard (so movement is real and stays).

## Why this and not engine work first

The audit re-measured the engine far ahead of its stale reputation (DOM core panic-free on both engines; CSS reftests 5-40x the circulated baselines). The remaining waste is that the runner discovers and scores the wrong set of tests, re-pays the testharness.js eval per test, and has no checked-in expectations, so nobody can trust a delta. Three harness levers fix that and are precondition for steering the CSS/DOM levers.

## Phases (done-conditions, not dates)

### H1 — MANIFEST.json reader (lever 1)

Replace the ad-hoc directory walk and heuristic expansion with the upstream-generated manifest.
- Landed: normal `list`, `run`, `reftest`, `dump`, `testharness`, and `compare` discovery now loads `tests/wpt/meta/MANIFEST.json` by default. `--walk-discovery` keeps the old raw directory walk only as a diagnostic fallback.
- Landed: each runner test carries both the WPT URL identity and the backing source file, so generated variants such as `dom/abort/event.any.html` resolve back to `event.any.js` while keeping the generated URL for listings, server mode, and expectations.
- Landed: manifest kind, variant URL, `timeout: long`, reftest references, and fuzzy metadata feed the runner. The old `.any.js` synthesizer remains only as the local disk wrapper builder for manifest-selected generated window variants; HTML fuzzy parsing remains a fallback when a non-manifest walk is requested.
- Done condition: met for the runner-owned hostable window subset. Worker/sharedworker/serviceworker/shadowrealm variants are explicitly skipped by capability, not silently counted.

### H2 — Snapshot-clone Runtime pool (lever 3)

Amortize the dominant per-test cost.
- Original diagnosis: each test built a fresh `Runtime` and re-evaluated the 5,207-line testharness.js. The bench probe (`harness.rs:393-414`) proved the eval, not `Runtime::new()`, was the dominant cost, that naive Runtime reuse leaked the `tests` singleton across re-evals, and prescribed a post-(harness-eval) snapshot cloned per test.
- Target shape: eval testharness.js once into a base agent, then snapshot-clone a fresh per-test agent from that state so each test starts post-harness-eval with a clean `tests` singleton.
- **Engine target (corrected 2026-06-25, grounded):** the runner scores on **Boa** by default, but the snapshot belongs in **Nova**. Per the conformance-target doctrine (improve **Nova**, keep **Boa** pristine as the oracle), Nova gets fast routine scored runs; Boa stays slow-but-pristine, run as the reference. Do **not** add a snapshot to Boa. The snapshot is an *optional per-engine capability* behind the `ScriptEngine` trait (a future V8 / SpiderMonkey / QuickJS brings its own, or none), so the harness must not assume any engine can clone.
- **2026-06-30 implementation spike:** a direct derive-based `GcAgent`/`Agent`/`Heap` clone probe in the Nova fork does **not** compile and should not be pursued as a mechanical patch. The blockers are structural:
  - `Heap` contains non-clone `SoAVec` arenas (`arrays`, `maps`, `sets`, `finalization_registrys`) and `soavec 0.2.0` does not implement `Clone` for `SoAVec`.
  - Core heap records lack `Clone`, including `ArrayBufferHeapData`, `ECMAScriptFunctionHeapData`, `ElementArrays`, `Environments`, `ObjectRecord`, `RegExpHeapData`, `ScriptRecord`, `SourceCodeHeapData`, `SourceTextModuleRecord`, and module `LoadedModules`.
  - A raw structural clone would be unsound even if it compiled: Serval's Nova host hooks carry the microtask job queue and module cache, while the realm `[[HostDefined]]` slot carries DOM host data, reflector roots, pending host promises, and the release queue. A snapshot clone must replace those with fresh per-clone host state.
- **Landed primitive (2026-06-30):** Nova now has an explicit `GcAgent::snapshot_clone` path with host-hook replacement and realm `[[HostDefined]]` reset semantics, plus Nova-side isolation tests. Serval exposes that as optional `ScriptEngineSnapshot`, `Runtime::snapshot_clone`, and a `NovaHarnessTemplate` that snapshots after `testharness.js` load so `--engine nova` reuses a post-harness heap per test without re-evaluating the harness source.
- **Empirical 2026-06-30 release probe:** the snapshot seam works on stable slices, but the broad `dom` corpus is still not Nova-safe enough for H2 to be considered complete. `serval-wpt testharness dom --engine nova` overflows the main-thread stack in release before producing a final aggregate, so the throughput win is currently gated by engine/runtime stability on the wide lane. On the smaller checked slices the shape is better: `dom/abort` completes at exact coarse parity with Boa in 1.38s vs 1.17s, `html/webappapis/timers` completes at exact coarse parity in 1.48s vs 1.62s, and `dom/nodes` completes with the same coarse buckets as Boa in 4.44s vs 34.36s.
- **Stack overflow resolved (2026-07-01):** isolated to `dom/events/AddEventListenerOptions-signal.any.js` — a test that re-dispatches an event from within its own listener, unbounded because the AbortSignal abort does not stop the re-entrant recursion. Boa throws a catchable `RuntimeLimit(Recursion)`; Nova's fixed 3500-execution-context cap (whose own comment admitted it "caused stack overflow") let the OS stack overflow first, since a re-entrant host callback burns far more native stack per level than a plain JS call. **Fixed two ways:** (1) the runner runs on a 512MB stack (serval `72b07ecf647`) — belt-and-suspenders, and it also covers serval's own non-Nova recursion (parser/layout); (2) **Nova's `check_call_depth` now measures *actual* stack use via a stack-pointer proxy** (address-of-local, anchored at the outermost JS entry) against a host budget (`AgentOptions::stack_limit_bytes`, default 768KiB), portable to native *and* wasm — `stacker` returns `None` on wasm, and a wasm shadow-stack overflow silently corrupts linear memory rather than trapping, so a proactive guard is mandatory there (nova `cce0f09b`). **Broad `dom --engine nova` now completes in ~12s at 90 all-pass / 2194 subtests — exact parity with the checked Boa `dom` baseline (88 / 2122).**
- **Done when (met 2026-07-01)** a broad release-mode `dom` run on Nova completes without the stack overflow, still preserves fresh per-test harness state, and keeps the measured per-test cost dominated by the test body rather than harness re-eval.

### H2b — Per-engine scoring + cross-engine diff (the Nova-improvement driver)

`run_test` is already generic over `E: ScriptEngine`, so scoring both engines on one corpus and diffing is a small addition with outsized value. A test that **passes on Boa but fails on Nova is a Nova JS-engine gap** (a fork improvement; watching this bucket shrink is the Nova-to-Boa gap closing). A test that **fails on both is a serval-platform gap** (layout / DOM, not the engine). This converts the scoreboard into a per-test worklist routed to the right owner, and operationalizes the audit's keep-Nova-80-and-Boa-94-distinct: Nova is the primary "are we improving" number, Boa the platform ceiling, `Boa − Nova` the Nova fork's remaining JS work. The two engines also map to the two PWA lanes (Nova/wasm64 on Chrome/Firefox; Boa/wasm32 on WebKit). Buildable on subsets today (no snapshot needed); H2a's snapshot is what makes it affordable on the full corpus.

- **Done when** a `compare <subset>` run reports the 2x2 (both-pass / both-fail / Boa-only / Nova-only) and the Boa-only set is surfaced as Nova's worklist.
- **Empirical (2026-06-25, `compare` landed):** across `dom/abort`, `dom/nodes` (302 tests), and `html/webappapis/timers`, **Boa and Nova are at exact parity — zero Boa-only (Nova) gaps**; every failure fails on *both* engines (e.g. dom/nodes 56 both-pass / 215 both-fail). So on WPT, **the failures are serval-platform** (DOM / layout / parsing — the audit's object-fit / interface-table / CSS levers), not the JS engine, and improving Nova moves nothing here. The Nova-vs-Boa gap is **ECMAScript language conformance (test262, which WPT excludes by design)**, so the **Nova worklist comes from a test262 runner scoring Nova, not from WPT-`compare`**. WPT-`compare`'s standing role is therefore **regression detection** (catch a future Nova divergence from the Boa oracle) and parity confirmation, not the Nova worklist. The manifest already carries a `test262` item type; a test262-`compare` (different harness: `$262` + frontmatter, not testharness.js) is the lever for Nova's actual gaps.

### H3 — Corpora re-score + checked-in expectations + regression guard (lever 5)

Turn measurement into a guardrail.
- Re-run hostable testharness subsets on H1 and publish current aggregates. Fetch remains a server-mode/netfetch lane because it needs `wpt serve`, the `netfetch` feature, and local host mapping; it does not belong in the default disk-mode guard. The CSS re-score is already carried by the CSS conformance scoreboard.
- Landed mechanism: `testharness` accepts `--write-expectations <file>` and `--expectations <file>`. The JSON format is per-test URL -> coarse status (`pass`, `fail`, `error`, `no-results`, `skip`) with optional pinned `reason`; the check fails on changed status, changed pinned reason, missing expectation, or stale expectation so `unexpected=0` is enforceable.
- Checked baselines:
  - `ports/serval-wpt/expectations/testharness/dom_boa.json` pins the full hostable `dom` manifest subset on Boa in release mode. **Corrected 2026-07-09 by reading the file:** 91 all-pass, 363 with-failures, 71 errored, 84 no-results, 51 skipped, over 660 tests. (The "88 / 365 / 72" figures quoted here and in the grand audit predate the file's 2026-07-05 rebase.) The errored + no-results set is triaged in H6.
  - `ports/serval-wpt/expectations/testharness/dom_abort_boa.json` keeps the focused abort slice pinned: 2 all-pass, 4 with-failures, 0 errored, 0 no-results, 3 skipped; subtests 29/37 passed.
  - `ports/serval-wpt/expectations/testharness/dom_nodes_boa.json` keeps the focused DOM-nodes slice pinned: 57 all-pass, 205 with-failures, 12 errored, 14 no-results, 42 skipped; subtests 1655/5365 passed.
  - `ports/serval-wpt/expectations/testharness/html_webappapis_timers_boa.json` pins the timer/event-loop smoke slice: 4 all-pass, 0 with-failures, 8 errored, 0 no-results, 0 skipped; subtests 7/7 passed.
- Separate server-mode baseline:
  - `ports/serval-wpt/expectations/testharness/fetch_api_basic_boa.json` pins the live-server `fetch/api/basic` slice on Boa in release mode with `--spawn-server`: 8 all-pass, 8 with-failures, 2 errored, 20 no-results, 0 skipped; subtests 81/119 passed.
- Local guard: `support/wpt/check-testharness-baselines.ps1` builds `serval-wpt` release and checks all listed baselines; `-NoBuild` reuses an existing release binary.
- Separate fetch guard: `support/wpt/check-testharness-fetch-baselines.ps1` builds `serval-wpt` release with `--features netfetch`, spawns `wpt serve`, and checks the live-server `fetch/api/basic` baseline. This lane remains local-only for now because it depends on local host mapping and takes several minutes per run.
- CI guard: `.github/workflows/wpt-harness.yml` runs the default disk-mode guard on push and pull request.
- **Done condition: met for the default hostable testharness lane.** A local or CI run now fails on changed, missing, or stale expectations.

## Sequencing

**H6 triage ran 2026-07-09** and reordered what follows. Of the 155 dom dead tests, only 6% are iframe-blocked, so iframe/second-realm drops down the list on `dom` evidence. 55% correlate with `test_driver`, but tracing one cluster showed a shim is the *last* of six prerequisites, not the lever (see H6) — so the leading candidates are now the DOM-conformance prerequisites it sits on (**window `EventTarget`, `onX` event-handler attributes, the `load` event**), which pay off well beyond WPT, plus a harness rAF pump. Those are engine work, so this plan's own remaining phases stay **H4 policy metadata plus H2 Nova broad-corpus stability**. H1/H3/H5 are done for their current lanes, and H2a's snapshot seam is wired. Fetch coverage is a separate server-mode/netfetch guard because the default disk-mode harness cannot own the required WPT server + host mapping setup.

## Non-goals

- Engine fixes (owned by the CSS conformance + HTML interface-table plans).
- A full `wpt serve` orchestration rewrite; the live-server fetch slice already works in server mode.
- iframe/second-realm execution (a larger harness capability; note it as a known wall, do not scope it here). **H6 sized this wall for `dom`: 9 of 155 dead tests.** So `dom` scoreboard recovery does not justify it; any case for iframe must now come from `html/` or `fetch/`, which H6 did not measure.
- `test_driver` input injection, previously unnamed here. It correlates with 85 of the 155 dom dead tests, but H6 traced the execution and found a shim is the last of six prerequisites: the tests die at `document.body.onload` long before reaching it. The real prerequisites (window `EventTarget`, `onX` handler attributes, the `load` event) are DOM-conformance work owned by the engine plans, not harness work. Only the rAF pump belongs here, and it is worthless alone.

## H4 — Governance: green-by-default, with sub-WPT micro-tests (from the formal-web harvest)

The gterzian/formal-web harvest (`2026-06-24_formal_web_lessons.md`) supplies a
governance model worth adopting alongside H3, turning the runner from a noisy
dashboard into a regression gate:

- **Skip-by-default `include.ini` + per-file opt-in**, and **`meta/*.ini`
  expected-result files** pinning expected pass/fail per sub-test with TODO
  reasons, so the default run asserts **`unexpected = 0`**. A new pass becomes an
  explicit metadata edit, not an invisible count drift. This is the policy layer
  over H3's aggregates + expectations guard.
- **Partial landing (2026-06-30):** the current JSON expectations format now
  accepts an optional per-test `reason` alongside the coarse status, and the
  runner emits explicit reasons for capability skips and early error classes
  (`non-testharness`, `xhtml`, `dedicated-worker-unsupported`,
  `sharedworker-unsupported`, `serviceworker-unsupported`,
  `shadowrealm-unsupported`, `non-window-global`, `read-failed`,
  `fetch-load-failed`, `panic`, `evaluation-threw`, `no-subtests`). This gives
  the harness an explicit policy vocabulary without yet adopting the full
  `include.ini` / `meta/*.ini` formal-web model.
- **Local deterministic micro-tests below the WPT level.** Small `.html` tests
  reporting via testharness.js OR a plain `window.__formalWebTestResult` object
  (same shape testharnessreport produces), mounted so they reuse upstream
  `/resources/testharness.js`. These let serval lock an event-loop / parser /
  streams milestone *before* the corresponding WPT directory is enabled — which
  matters now, while whole directories are gated by the H1/H2 work. (The
  `byob-debug.html`-style micro-test is the model; see the BYOB streams plan.)

**Optional rigor capability — TLA+ trace validation of the scheduler.** Distinct
from WPT, and architecture-agnostic (it needs only an event log + a model).
The first fixture witness is now wired under `docs/tla/scheduler_trace/` with a
checked-in generated `SchedulerTraceData.tla`, a generator drift check, and CI TLC
wiring. That plan completed and archived 2026-07-05
(`archive/2026-06-24_event_loop_rigor_plan.md`); broader protocol specs are now
carried by `2026-07-05_event_loop_rigor_followups.md`, not a blocker for this
harness plan.

## H5 — test262 runner (the Nova worklist, realized)

H2b's finding (WPT excludes ECMAScript, so the Nova-vs-Boa gap lives in test262) made a
test262 runner the actual Nova-improvement lever. The full corpus (53,166 tests) is vendored
at `tests/wpt/tests/third_party/test262`.

- **Built** (`ports/serval-wpt/src/test262.rs` + `main.rs`): frontmatter parse (includes /
  flags / negative / features), harness assembly (assert.js + sta.js + includes, strict
  variants, raw), per-engine run + pass/fail vs `negative:`, and `test262 <subset>` running
  **both engines and diffing** (Boa-pass / Nova-fail = a Nova gap). Module tests run via
  `eval_module` (harness preamble as a sloppy script, then the test as a module). Negative
  tests match the **expected error type** (`ScriptEngine::describe_error` — Boa stringifies
  the opaque `JsError` via `into_opaque`+`toString`; Nova's `Error` is already the message),
  not merely "threw".
- **Hang-safety (load-bearing):** Boa and Nova **cannot be step-metered** (`eval_bounded` is
  unbounded for both; only a fuel-metered backend like piccolo could), so a pathological test
  (an infinite loop) would stall the whole run, and an in-process watchdog can't interrupt a
  spinning eval. The runner isolates **each test in a worker subprocess** (`test262-one`) with
  a wall-clock `--timeout` (default 30s): a hang kills only that process, is recorded as a
  timeout **attributed to whichever engine never reported**, and the run continues. A pool of
  `nproc` workers pulls from a shared atomic work index. This *is* the parallelism (it
  subsumes the earlier in-process thread-scope) and the affordability lever for test262, so
  test262 corpus safety does not depend on H2a. H2a later landed for the WPT/Nova testharness
  throughput lane. (jemalloc is already linked; per-test cost is engine-bound, ~0.1s subprocess
  startup is the price.)
- **`async` is re-enabled (2026-07-01):** the earlier corpus-scale memory blow-up was two
  *non-async* Promise infinite-hangs spinning in shared threads, not async. Per-test
  subprocess isolation bounds each async test to its own reaped process, so `run_262_async`
  (`$DONE` via a `print`→buffer shim + `run_event_loop` + the `AsyncTestComplete` sentinel)
  is safe. The remaining skips are missing-includes + the few async+module combos.
- **Done when** a full-corpus run completes without stalling and writes a triageable Nova
  worklist (`--worklist-out`). **Done.**

### Full-corpus result (2026-06-29, all 53,166 tests, 30s timeout)

```text
both-pass=35858  both-fail=3374  boa-only=7818 (Nova gap)  nova-only=513  timeout=21  skipped=5582
```

Of the 47,563 that ran on both engines: **Nova 76.5%, Boa 91.8%** (matches the audit's
~80/~94 in shape). The **7,818-test Nova worklist** is overwhelmingly concentrated:

- **Temporal = 5,873 (75% of the entire gap)** — built-ins/Temporal 3,967 + intl402/Temporal
  1,896. Completing Temporal in Nova closes three-quarters of the Nova-vs-Boa gap; it is THE
  convergence lever.
- Next tiers: RegExp 225, staging/sm 221, Iterator-helpers 217, Set-methods 152, Promise 141.
- **Language-core (533)** — the more fundamental gaps (not proposal builtins): literals/regexp
  172, statements/with 107, class 106, compound-assignment 44 (`&&=`/`||=`/`??=`), `using` 18.
  Smaller count, higher correctness priority than proposal-stage builtins.

**Timeouts (21), attributed by engine** — the runner doubles as a hang/perf-cliff finder:

- **13 Nova**, including a **systematic Promise iterator-close hang family** (`Promise.all` /
  `allSettled` / `race` × `invoke-then-error-close` / `invoke-then-get-error-close` — 6 tests,
  one infinite-loop root cause), plus perf cliffs (Array/defineProperty `length`,
  `decodeURI`/`decodeURIComponent`, a Date caching test).
- **8 Boa** — `staging/sm/Date/dst-offset-caching` (7) + `String/replace-math` (1). So **Boa
  is not a flawless oracle** (the 513 nova-only confirm it); the diff catches both directions.

## H6: dom errored / no-results triage (scoreboard recovery + iframe-lever sizing)

Added 2026-07-09. **Ran 2026-07-09; the result re-ranks the WPT levers.**

The checked `dom_boa.json` baseline pins **71 errored + 84 no-results = 155**
hostable `dom` tests contributing **zero scored subtests** on the scored default
engine. They fail *before* any engine correctness is measured, so recovering them
is harness work, exactly H3's "re-score restores prioritization" doctrine applied
to the tests that score nothing.

*Number correction:* the grand audit and H3 above quote `dom_boa.json` as 88
all-pass / 365 with-failures / 72 errored. Those are stale against the file's
2026-07-05 rebase, which actually holds **91 pass / 363 fail / 71 error / 84
no-results / 51 skip over 660 tests**. Trust the file, not the prose.

**Why it went first:** the load-bearing question was how much of the 155 sits
behind the iframe/second-realm wall, since that wall's payoff cannot be sized
without it (see Non-goals). The answer is: almost none.

Each dead test was resolved to its source file (generated `.any.html` /
`.window.html` variants resolved back to the backing `.js`) and classified by the
capability it loads:

| Count | Share | Blocked on |
| --- | --- | --- |
| 85 | 55% | **`testdriver.js` / `testdriver-actions.js`** — WebDriver-style input injection |
| 60 | 39% | no capability marker: ordinary engine/platform gaps (`nodes` 23, `ranges` 22, `events` 9, `traversal` 3) |
| 9 | 6% | second-realm: iframe / `srcdoc` |
| 1 | 1% | frameset / `beforeunload` |

**Findings, in the order they change decisions:**

1. **iframe/second-realm is not the `dom` lever.** It gates **9 of 155**. The
   audit's claim that the wall "walls off chunks of fetch/ and most of html/" is
   *untested by this triage*, which is `dom`-only, and may still hold there. But
   iframe can no longer be justified by `dom` scoreboard recovery.
2. **`testdriver` correlates with the dead set (85 tests), but a `test_driver`
   shim is *not* the unlock.** *Corrected 2026-07-09 after tracing the actual
   execution of `dom/events/non-cancelable-when-passive` (40 tests, every one
   `no-results`).* Those tests call `runTest` from `document.body.onload`, then
   `await` two `requestAnimationFrame` turns, then `test_driver.Actions()…send()`,
   then poll via rAF. Six layers must exist before the last one matters, and
   serval has **none** of the first four:
   - ~~**No window `EventTarget`.**~~ **Corrected 2026-07-10:** the testharness
     lane *does* have one. `EVENT_TARGET_BOOTSTRAP` (`script-runtime-api/lib.rs`)
     gives `globalThis` add/remove/dispatchEvent over a standalone `EventTarget`
     (plus `UIEvent`/`MouseEvent` constructors). It is detached from the DOM tree
     (dispatching at a node does not reach it except via the propagation-path
     special case), but it is enough for `testharness.js`'s own listeners. The
     general bootstrap outside this lane still lacks it.
   - **No event-handler IDL attributes.** There is no `onX` property mechanism at
     all, so `document.body.onload = fn` sets an inert expando. `runTest` is never
     called, so `promise_test` never registers: hence `no-subtests`, before
     testdriver is ever reached. **This, not testdriver, is the cluster's first
     blocker** (corrected ranking, 2026-07-10).
   - ~~**No `load` event.**~~ **Corrected 2026-07-10:** `run_loaded_testharness`
     dispatches `window.dispatchEvent(new Event('load'))` after test eval
     (`lib.rs:474`). The original claim came from a grep quoting `"load"` while
     the code has `'load'` inside an eval string. What remains true: the `load`
     dispatch reaches only the window shim's listeners, not `body.onload` (no
     `onX`) and not node-tree `load` listeners.
   - **No rAF pump in the harness.** True and unchanged:
     `Runtime::run_animation_frame_callbacks` exists but nothing in the lane
     calls it (`run_event_loop` is timers + microtasks only), so
     `waitForCompositorCommit` / `waitFor` can never progress.
   - Only then: no `test_driver_internal` backend (WPT's `testdriver-vendor.js` is
     deliberately blank; the vendor supplies it), and no Touch/Pointer synthesis
     with the passive/`cancelable` semantics these tests actually assert.

   So the ordered prerequisites are: window EventTarget → `onX` handlers →
   `load` → harness rAF pump → `test_driver_internal` → touch/pointer events.
   The first three are ordinary DOM conformance work with payoff well beyond WPT;
   the shim by itself buys nothing. **Do not scope "a test_driver shim" as a
   lever.**

3. **60 are plain engine gaps**, not harness gaps. They already belong to the
   DOM/engine levers and should be routed there, not counted as harness debt.

4. **The harness half of this is one capability, and it gates three separate
   plans** (found 2026-07-09 while wiring the CSS-animations WPT slice). The
   `testharness` lane builds a `Runtime` over a `StaticDocument` and **never
   constructs an `IncrementalLayout`** — no animation clock, no `tick_animations`,
   no rAF pump, no `load` event. Consequences, all the same root cause:
   - the 85 `testdriver` dom tests cannot run (they need `load` + rAF);
   - the `css/css-animations` event-order and interpolation-over-time tests cannot
     pass, so that corpus is pinned status-only
     (`2026-07-09_css_animations_plan.md`, A3);
   - the CSS **transitions** plan's T3 WPT slice was never wired, for exactly this
     reason (`2026-07-05_css_transitions_plan.md`, T3).

   A driven rendering loop in `serval-wpt` (construct a session over the test DOM;
   per turn: fire `load` once, drain rAF callbacks, tick animations, harvest and
   dispatch transition + animation events) is the single harness capability that
   unblocks all three. It belongs to this plan; the DOM prerequisites above do not.

5. **The fifth link in item 2's chain, `test_driver_internal`, has an owner:** the
   native automation plan (`2026-07-09_native_automation_plan.md`, finding 11). Its
   phase-1 core is reachable **in-process**, because `test_driver` is an embedder
   hook rather than a protocol: `testdriver.js` routes every command through
   `window.test_driver_internal`, whose shipped defaults throw and whose
   `testdriver-vendor.js` is a blank file by design. So `serval-wpt` supplies that
   object and binds it to the core, the way the runtime already exposes
   `__matchMedia` / `__dispatchSynthetic`. No HTTP and no WebDriver adapter on this
   path; the adapter is needed only to run WPT's own `webdriver/` conformance
   suite. Two constraints fall out, both now recorded in that plan: the WebDriver
   Actions tick interpreter must live in the core (it is what `action_sequence` is
   handed), and the core's actuate side must be defined against a surface seam
   rather than winit, because this lane is headless. The sixth link
   (`TouchEvent` / `WheelEvent`, absent from `dom/bootstrap.js`, though `passive`
   listener options already exist) stays DOM work.

## H7: driven rendering loop + the test_driver hookup (claimed 2026-07-10, WPT lane)

**Coordination record, answering the automation lane's question.** Their Actions
tick interpreter landed (`aec5fee11d1`,
`shared/embedder::webdriver_actions::interpret_actions`) and was verified against
this consumer's needs: element origins resolve through a caller-supplied closure
(here, the harness session's geometry) and tick durations are **reported, never
slept** (here, mapped onto the harness's clock). Both purity rules are exactly
what a headless consumer requires. **The hookup (H7b) is this plan's lane**, per
their suggestion to coordinate rather than land it unilaterally; `ports/serval-wpt`
is this plan's edit surface. Their remaining phase-1 items (condition-wait etc.)
touch only their crates and can proceed in parallel with no hazard.

### H7a — the rendering loop

Give the testharness lane the layout session it has never had. Construct an
`IncrementalLayout` over the runtime's live DOM (the pattern serval-scripted's
tests already prove: session + `ComputedStyleBridge`); back `getComputedStyle`
with it. Each drive turn: drain DOM mutations -> `apply`, run rAF callbacks
(`run_animation_frame_callbacks`), `tick_animations`, then
`take_transition_events` + `take_animation_events` -> dispatch through the
runtime. Pacing: never quiesce while rAF callbacks or active animations pend
(the deadline still backstops); in disk mode drive the clocks **virtually**
(timers, rAF, and the animation clock all take a caller-supplied `now`), so a
2s animation costs evaluation time, not wall time.

**Done when** `css/css-animations` moves: the event-order / interpolation tests
score instead of erroring, `css_animations_boa.json` is deliberately rebased
upward, and every other checked baseline stays `unexpected=0` or is deliberately
rebased with the delta named.

**Landed 2026-07-10.** The session + virtual-clock drive is in
(`RenderSession` + `drive_virtual` / `drive_wall` in `harness.rs`); every
testharness run now gets it. Results, honestly stated:

- **The event machinery works end to end**: `animationevent-types.html` (three
  async listeners: start / iteration / end, negative delay, animates `left`)
  reports 0/3 *scored* subtests where it previously died unscored — all three
  listeners fire; the asserts fail on genuine platform gaps (`evt instanceof
  AnimationEvent` — the bootstrap constructors are factory-shaped, not
  prototype-chained — and Web Animations' `.animation` attribute). Guard:
  `animationevent_types_survives_the_rendering_session` (harness) +
  `negative_delay_and_the_f32_boundary_tick_survive` (serval-layout).
- **Deliberate rebases, deltas named**: `dom` and `dom/nodes` each +1 all-pass
  (`dom/nodes/moveBefore/preserve-render-blocking-script.html` fail -> pass,
  subtests 2230 -> 2233); `html/webappapis/timers` 4 all-pass / 8 errored ->
  **6 all-pass / 4 with-failures / 2 errored**, subtests 7/7 -> 9/14 — the
  far-future-`setTimeout` family (`type-long-settimeout` etc.) became runnable
  because the virtual clock jumps to a huge delay instead of never reaching it.
  `dom/abort` and `css/mediaqueries` unchanged at `unexpected=0`.
- **`css/css-animations` aggregates did not move** (156/1198, statuses
  unchanged): the tests the loop unlocked were already coarse-`fail`, and their
  remaining asserts need engine surface, not harness. The next levers for that
  corpus, in observed order: (a) prototype-chain the `AnimationEvent` /
  `TransitionEvent` bootstrap constructors so `instanceof` holds; (b)
  `computed_query` longhand coverage — `computed_value(_, "left")` returns
  `None` today, and interpolation tests assert positional longhands via
  `getComputedStyle`; (c) Web Animations API surface (out of scope).
- **The stylo fork gained a load-bearing fix** (`mark-ik/stylo` `56e70cacdb`,
  serval lock repinned): Stylo's keyframe search casts f64 progress to f32
  against f32 start percentages, so a progress in `(1 - 2^-24, 1.0)` — which an
  accumulated 16.667ms frame clock produces routinely — fell into a
  `debug_unreachable` and panicked the process. Six css-animations tests panicked
  this way on the loop's first run. **Any consumer that ticks Stylo animations on
  an accumulated clock wants this fix; mere pins its own stylo rev and should
  repin.**
- Cost: full `dom` (660 files) runs in ~2m07 release with per-test sessions;
  `css/css-animations` ~5m30 (it genuinely drives animations now).
- Noted in passing: `webgl_conformance::gl_clear_conformance_runs_through_the_harness`
  fails on HEAD *before* this work ("not a callable function" in the Khronos
  shim) — pre-existing, owner unknown, not chased in this lane.

### H7b — `test_driver_internal` over the interpreter

Supply `window.test_driver_internal` in the harness surface;
`action_sequence` -> `interpret_actions` with a session-geometry resolver; per
tick, advance the harness clock by `duration_ms` and dispatch the tick's
`InputEvent`s as synthetic DOM events at hit-tested nodes. Sequenced after H7a
(the resolver and the hit-testing *are* the session). The touch cluster
additionally needs `onX` handler attributes and `TouchEvent` (DOM work, links
2 and 6 — not this plan's).

**Landed 2026-07-10.** The seam is WPT's own: `collect_scripts` splices serval's
backend (`TESTDRIVER_VENDOR_JS`) in place of the deliberately-blank
`testdriver-vendor.js`, in document order — after `testdriver.js` defines the
throwing defaults, before any test script can call them. The vendor sets
`in_automation = true` (unimplemented commands now throw spec-honestly instead
of waiting forever for a human), normalizes element origins to the wire
element-reference format, and queues transactions the drive loop drains
(`process_testdriver_actions`): parse with the pinned `webdriver` protocol
types, run through the shared `embedder_traits::webdriver_actions::interpret_actions`,
resolve element origins through the session's `absolute_rect`, dispatch each
tick's mouse events at `hit_test`-ed nodes (pointerdown/mousedown,
pointerup/mouseup/click, pointermove/mousemove — the interpreter's own
mouse-only first cut), advance the virtual clock by the tick's reported
duration, render a turn, settle the Promise.

- **One runtime addition was needed:** `__nodeRawId(ref)`
  (`script-runtime-api/dom/tree.rs`), the reverse of `__reflectNode` — the
  wrapper's `__ref` is a JS-opaque reflector, so handing a node *back* to Rust
  needs the native id extraction (found the hard way:
  `String(el.__ref)` serialized as `"[object Object]"` and the interpreter
  correctly refused the origin rather than guessing).
- **Guard, and the first cross-consumer test of the automation core:**
  `test_driver_action_sequence_synthesizes_a_click` — real `testdriver.js`, the
  vendor seam, Actions-format JSON, the interpreter, session geometry, and a
  synthesized click completing an `async_test`, with zero coordinate literals
  and zero sleeps.
- Not yet: keyboard/wheel/touch dispatch (interpreter emits key events; the
  bridge skips them), event coordinates on the synthetic events
  (`dispatch_event` carries type only), and the remaining testdriver commands
  (`click()`, `send_keys()` keep their throwing defaults — spec-correct
  `unsupported` rather than fakes).

## H8: DOM event surface — `onX` handler attributes + typed event interfaces (2026-07-10)

The DOM-side prerequisites (H6's chain links 2 and 6), which the harness plan
does not own but which the harness lane is the reason to prioritize. Both are in
`script-runtime-api` (the runtime), not `serval-wpt`.

### H8a — event-handler IDL attributes (`el.onclick = fn`, link 2)

The 40-test `dom/events/non-cancelable-when-passive` cluster registers via
`document.body.onload = () => runTest(...)`; with no `onX` mechanism that set an
inert expando and `runTest` never ran (the cluster's true first blocker, ahead of
`test_driver`). Landed in `dom/bootstrap.js`: a getter/setter per handler name
managing one stable listener (registered once on first non-null assignment,
calls the current value, so reassignment keeps registration order and `= null`
is a no-op). The **WindowEventHandlers set** (`load`, `resize`, `scroll`,
`error`, `blur`, `focus`, …) **reflects from `<body>`/`<frameset>` onto the
Window** — which is how `body.onload` catches the `load` event the harness
dispatches *at the window*. Guard:
`event_handler_idl_attributes_work` (boa + nova) — element handler + coexisting
`addEventListener` listener fire in set order, `body.onload` reflects to window,
`= null` removes the handler while the listener survives.
- Deferred: the **content-attribute** form (`<body onload="run()">` compiled
  from the attribute string). Common in older WPT tests; a separate compile step.
  The IDL form is what the passive/cancelable cluster and most modern tests use.

### H8b — typed event interfaces (`TouchEvent` / `WheelEvent` / …, link 6)

`TouchEvent`, `WheelEvent`, `PointerEvent`, `KeyboardEvent`, plus `Touch` /
`TouchList`, added to the event bootstrap (`lib.rs`) prototype-chained like the
existing `MouseEvent`: correct fields, `cancelable` honored, `instanceof` holds
through the chain (`TouchEvent → UIEvent → Event`, `WheelEvent → MouseEvent →
UIEvent → Event`). The passive/cancelable cluster reads `event.cancelable` off a
`touchstart`, so the type and its flag must both be real. Guard:
`typed_event_interfaces_work` (boa + nova).
- **Honest scope limit — the cluster still won't fully pass from H8 alone.** The
  types now exist, but a touch-pointer `test_driver.Actions()` must actually
  *produce* `TouchEvent`s at the target, which needs (i) the shared interpreter to
  emit touch (not mouse) events for `pointerType: 'touch'` — it is mouse-only
  first-cut, the automation lane's call — and (ii) H7b's bridge to construct a
  typed `TouchEvent` with the right `cancelable` rather than a bare `Event` by
  type. Both are follow-ons. H8 removes the two DOM blockers; the input path is
  the remaining one.
- **`ontouchstart` and friends are deliberately NOT exposed.** Per Touch Events,
  the `on*` touch IDL attributes exist only when "expose legacy touch event APIs"
  is true (a touch-capable device). Exposing them regressed
  `Document-createEvent-touchevent` (which branches on
  `'ontouchstart' in document`) from pass to fail. Touch listeners still work via
  `addEventListener`; only the `on*` reflection is gated. The `TouchEvent`
  *interface object* stays defined.

### H8c — listener exceptions are reported, not propagated (found by H8a)

A DOM conformance bug the `onX` work exposed, and a load-bearing one: a
listener's exception was **propagating out of `dispatchEvent`**. Per DOM
§dispatch it must be *reported* — dispatch continues to the remaining listeners
and `dispatchEvent` returns normally. Because H8a made `onload` handlers actually
run, this would have spuriously errored out whole test files across the corpus
(it already had: it turned `Element-getElementsByTagName-change-document-HTMLNess`
from `fail` into `error`).

Fixed in both dispatch paths (`fire` in `dom/bootstrap.js`, `EventTarget.__fire`
in `lib.rs`): the callback is invoked under try/catch, and the exception goes to
`__reportListenerException`, which fires an **ErrorEvent-shaped `error` event at
the global** and falls back to the console. That shape is deliberate:
`testharness.js` does `addEventListener("error", …)` and reads
`message` / `error` / `filename` / `lineno` / `colno` to fail the *running test*,
so a throwing handler now fails its test instead of killing the file. Recursion
guarded (a throwing error-listener cannot re-enter). Guard:
`listener_exceptions_are_reported_not_propagated` — dispatch continues past a
throwing listener and the exception surfaces as an `error` event.

### H8 measured result (2026-07-10)

**The 40-test `dom/events/non-cancelable-when-passive` cluster is alive.** All 40
moved from `no-results` (dead — never registered a subtest, because
`document.body.onload` set an inert expando) to running. `dom` overall:

| | before H8 | after H8 |
| --- | --- | --- |
| all-pass | 92 | **96** |
| no-results (dead) | 84 | **42** |
| subtests | 2233 | **2244** |

- **44 tests revived** (`no-results` -> `fail`): 40 in the passive/cancelable
  cluster, 3 in `dom/events/scrolling`, 1 beforeunload. They now register and run;
  they fail on the touch-input gap above, which is the honest next blocker.
- **4 newly passing**: `EventTarget-dispatchEvent`, `KeyEvent-initKeyEvent` (the
  `KeyboardEvent` interface now exists), `window-composed-path`, and
  `non-cancelable-when-passive/synthetic-events-cancelable` — the one member of
  the cluster that dispatches *synthetic* events rather than needing real touch
  input, so it passes end to end today.
- `css/css-animations`: 3 more tests revived (`no-results` -> `fail`), subtests
  157/1208.
- **One lateral move, named:** two `dom/ranges/Range-in-shadow-*` variants went
  `error` -> `no-results`. They are shadow-DOM-blocked either way (they call
  `attachShadow` inside a `load` handler) and score zero subtests in both states;
  the change is a direct consequence of H8c correctly *not* propagating the
  listener's exception. Residual fidelity gap noted: serval's results bridge does
  not surface testharness's *harness-level* error status, so a test whose only
  failure is a thrown handler with no registered subtests reads as `no-results`
  rather than a reasoned error. Separate item.
- All six checked baselines re-verified `unexpected=0` after the deliberate
  rebase of `dom` and `css/css-animations`.

## H9: the input path, end to end (2026-07-10)

The last link. The cluster H8 revived now **passes**: `dom/events/non-cancelable-when-passive`
is **38 of 42 all-pass** (was 0), and `dom` all-pass went **96 -> 144** (+48
`fail` -> `pass`, zero regressions). The four stragglers are a layout gap, not an
input gap (below).

### What it took

1. **Interpreter: touch pointers emit touch events** (`webdriver_actions.rs`,
   the shared crate — coordinated with the automation lane, whose first cut was
   mouse-only). `pointerType: "touch"` now emits `InputEvent::Touch`; mouse and
   pen still emit mouse events. Two rules fall out and are load-bearing:
   - **A touch that is not down emits nothing on a move.** There is no hovering
     finger, and the spec's own `injectInput` idiom moves the pointer to its
     origin *before* pressing — that move must not fabricate a `touchmove`.
     Position is still tracked, so the press lands right.
   - A touch that never went down cannot lift.
2. **Bridge: typed events, and the touchstart target** (`serval-wpt/harness.rs`).
   `InputEvent::Touch` / `Wheel` now go through typed dispatch. Per Touch Events,
   **every event for one touch point goes to the element the touch started on**,
   not a fresh hit-test, so the bridge remembers that target per touch id for the
   transaction. Wheel dispatches both the standard `wheel` and the legacy
   `mousewheel` (WPT covers both), un-negating the interpreter's sign so the DOM
   event carries the spec's.
3. **The `cancelable` rule, which is the whole point of the cluster.** A
   UA-generated touch/wheel event is cancelable **only if a non-passive listener
   for its type exists on the propagation path** — the passive-listener
   optimization. Only the DOM knows the listener set, so `__dispatchTouch` /
   `__dispatchWheel` (bootstrap) walk the path and decide. Deliberately scoped to
   the UA input path: a *script*-dispatched event keeps whatever `cancelable` its
   constructor was given (`generic-events-stay-cancelable` pins this, and passes).
4. **`window.innerWidth` / `innerHeight`** (`HostState::viewport_size` +
   `Runtime::set_viewport_size` + natives, the `scrollX` seam pattern). The wheel
   half computes its hit point as `Math.floor(window.innerWidth / 2)`; without
   these it scrolled at `NaN`. One `VIEWPORT_W/H` constant now feeds the layout
   session, `matchMedia`, and `innerWidth` so they cannot drift.

### Two pre-existing DOM bugs this surfaced, both load-bearing

- **Window listeners never fired for bubbled events.**
  `globalThis.addEventListener` delegated to a *private* `EventTarget` instance,
  so the window's listeners lived on an object `Node.prototype.dispatchEvent`
  never looked at (it fires the top of the path by reading
  `globalThis.__listeners`). So `window.addEventListener('click', …)` never saw a
  click on the page — only events dispatched *at* the window. The same wrapper
  silently dropped its third argument, so `{passive}`/`{capture}`/`{once}` never
  applied to a window listener. Fixed by making `globalThis` itself the window
  EventTarget. This alone was 8 of the cluster's tests. Guard:
  `window_listeners_receive_bubbled_events`.
- **`cancelBubble` set before dispatch was silently un-set.** `dispatchEvent`
  cleared the stop-propagation flags at the *start*; the DOM clears them *after*
  (§dispatch), so an event stopped before dispatch must fire nothing. The
  existing `cancel_bubble_before_dispatch` test had been passing **vacuously** —
  it asserted "window did not fire", which was trivially true while window
  listeners were unreachable. Fixing the window bug exposed it. Both dispatch
  paths corrected.

### The 4 stragglers: a `position: fixed` layout gap, precisely diagnosed

`wheel` / `mousewheel` on-`div` fail because their `#div` is
`position: fixed; top:0; right:0; bottom:0; left:0` with no width/height, and
serval resolves a **fixed** box's insets against its **parent** instead of the
**viewport** (the ICB). Probed directly: the div computes to `(0, 0, 800, 0)` —
width 800 resolves fine from `left:0`+`right:0`, but height is 0 because the
parent `body` has `height: auto`, which is 0 (its only child is out of flow). Give
`body` an explicit height and the same div resolves to `(0, 0, 800, 600)` and
hit-tests correctly. So inset-derived sizing *works*; the containing block is
wrong.

Confirmed further: serval-layout has **no fixed-vs-absolute handling at all** — it
hands stylo's style to `stylo_taffy::TaffyStyloStyle` and inherits Taffy's
absolute positioning, which is parent-relative by construction. `fixed` and
`absolute` are indistinguishable today. Closing it means *introducing* the
containing-block concept (ICB for `fixed`, nearest positioned ancestor for
`absolute`), with paint-order and scroll consequences. **Owned by the layout lane,
not this plan** — filed in `2026-06-16_serval_layout_roadmap.md`'s near-horizon
threads with the full diagnosis. It is the only thing between this cluster and
42/42, and it mis-sizes any fixed overlay (sticky header, modal backdrop) whose
parent has an auto height, so it is not a WPT-only curiosity.

**Resolved 2026-07-11:** the layout lane's F1 landed
(`2026-07-11_position_containing_block_plan.md` — fixed boxes hoist to the ICB
in the box tree, with the hit walk taught the same containing-block escape).
The wheel quartet passed, making the cluster **42/42 all-pass, 53/53 subtests**,
and `dom` rebased to 148 all-pass at `unexpected=0`. H9 is closed end to end:
every test the H6 triage attributed to the input path now runs and passes.

**H4a's `reason` vocabulary cannot express this triage.** The plan assumed the
155 could be "bucketed by pinned `reason`". Verified against a live
`--write-expectations` run: every one of the 155 carries exactly one of two
reasons, `no-subtests` (84, all the no-results) or `evaluation-threw` (71, all
the errors). Those name the *symptom*, not the cause, and they cut across the
capability buckets rather than aligning with them:

| | `no-subtests` | `evaluation-threw` |
| --- | --- | --- |
| testdriver | 58 | 27 |
| no marker | 24 | 37 |
| iframe | 2 | 7 |

So the capability classification above is the load-bearing artifact, and the
reason field adds nothing to it. Notably a `testdriver`-blocked test does **not**
reliably land in `no-subtests`: a third of them throw instead.

- **Done when** ~~the 155 are bucketed by pinned `reason`~~ **done 2026-07-09**
  via the capability bucketing above (the reason field turned out to be too
  coarse; see the cross-tab). Residual, to make the buckets a checked artifact
  rather than a one-off analysis: teach the runner a `testdriver-unsupported`
  reason (detect `/resources/testdriver.js` in the test source) and rebase
  `dom_boa.json` with it pinned. That touches a CI-enforced baseline, so it is
  its own change.

## Findings

- Historical 2026-06-24 finding (from the grand audit, adversarially verified at the time): the runner was 2,770 LOC (`main.rs` 1,671), had no MANIFEST reader in the run path, and had no checked-in expectations guard. This is now stale for the default hostable testharness lane: manifest discovery is wired into normal commands, JSON expectations exist, and the checked baselines run under a local/CI guard.
- `harness.rs` bench prescribed the snapshot-clone pool; H2a is now implemented as a Nova-owned `snapshot_clone` capability and a `NovaHarnessTemplate` in `serval-wpt`. The remaining finding is empirical, not architectural: broad release-mode `dom` on Nova still overflows the main-thread stack before a final aggregate.
- fetch/ runs only behind an off-by-default feature and the server-mode lane (`--spawn-server`). The checked `fetch/api/basic` guard remains local because it depends on WPT server startup and host mapping. Testharness XHTML/.xht files are skipped; XML reftest handling is separate. CSP, websockets/, and h3 are unrunnable through the runner despite netfetcher shipping the transports.
- The "re-score floats/normal-flow/css-backgrounds" sub-lever is already largely done inside the CSS conformance doc's scoreboard; H3's residual value is fresh dom/fetch aggregates + the expectations guard, not re-scoring CSS from scratch.

## Progress

- 2026-06-24 — Plan created from the grand audit. No code yet. H1 is the entry point.
- 2026-06-25 — **H1 reader + `manifest` command landed** (serval `a9703342ecd`): a MANIFEST.json reader (`ports/serval-wpt/src/manifest.rs` — URLs / kind / refs / fuzzy / pre-expanded variants; unit-tested + integration-tested against the real ~39MB manifest) and `serval-wpt manifest <subset>`. **Validated vs the walk on `dom/nodes`:** manifest 319 runnable (testharness 302, reftest 3, crashtest 14) vs walk 342 — the walk over-counts (38 `load` + 2 `reference` non-tests) and under-counts variants (+17 testharness), confirming the heuristic enumeration scores the wrong set. Additive (the run path still walks; slice 3 wires the manifest through it).
- 2026-06-25 — **H2 corrected** (above): the snapshot goes in **Nova**, not Boa (Boa is the pristine oracle); added **H2b** (per-engine scoring + cross-engine diff) as the Nova-improvement driver.
- 2026-06-25 — **H2b `compare` landed** (serval `c27d98d4145`): runs each testharness test on Boa + Nova and routes failures (both-fail = serval-platform, Boa-only = Nova gap). **Finding:** Boa/Nova at exact parity on `dom/abort`, `dom/nodes`, `html/webappapis/timers` (0 Nova gaps); WPT failures are serval-platform, so the Nova worklist is a **test262** matter, and WPT-`compare`'s role is regression-detection. Gotcha: run the runner in **release** (debug frames overflow the stack on bounded-deep recursion; the audit's "panic-free on both engines" holds in release).
- 2026-06-28 — **H5 test262 runner landed**: core + run-path + cross-engine `test262 <subset>` (`5df84ab9e23`, `d133f56350f`), confirming the H2b finding empirically (`built-ins/Temporal/Now` 66/66 boa-only — Nova lacks Temporal; `optional-chaining` at parity). Parallelized across cores (shared work-index, `8a5a393b1bb`); **measured ~3.2x, not the hoped ~14x** — jemalloc (`servo-allocator`) is already linked, so the ceiling is memory-bandwidth + per-test agent churn, not the allocator. Module support via `eval_module` (`d149fc649e5`); negative **error-type** matching via `ScriptEngine::describe_error` (`016dffea9fd`); `--worklist-out` full dump (`1b5feddb8d8`).
- 2026-06-28 — **hang-safe runner** (`90ae2edc268`, `aefc0db6103`, `f690f364f22`): the engines can't be step-metered (`eval_bounded` is unbounded for Boa+Nova) and a *non-async* `Promise.race` iterator-close test hangs serval, so a corpus run needs **per-test worker-subprocess isolation** + kill-on-`--timeout` (default 30s), not an in-process watchdog. Each timeout is attributed to the engine that never reported. This subsumes the parallelism for test262 and makes test262 corpus-safety independent of H2a. `async` was built + validated per-test but reverted (corpus-scale memory accumulation; pending investigation).
- 2026-06-29 — **full corpus run** (53,166 tests): `both-pass=35858 both-fail=3374 boa-only=7818 (Nova gap) nova-only=513 timeout=21 skipped=5582` → Nova 76.5% / Boa 91.8%. **The Nova worklist is 75% Temporal** (5,873 of 7,818); next tiers RegExp/Iterator/Set/Promise + a 533-test language-core tail; see the H5 result section. Found a systematic Nova Promise-combinator iterator-close **hang family** (6 tests) and perf cliffs on both engines (Nova: Array/defineProperty `length`, `decodeURI`; Boa: `Date/dst-offset-caching`).
- 2026-06-29 — **H1 normal-runner wiring + H3a expectation mechanism landed**: `list`, `run`, `reftest`, `dump`, `testharness`, and `compare` now discover from MANIFEST.json by default; `--walk-discovery` keeps the old heuristic path as a diagnostic fallback. Manifest-selected generated `.any.html` variants retain their WPT URL while resolving scripts from the backing `.any.js` file. Reftest refs/fuzzy come from the manifest with HTML parsing as fallback. `testharness --write-expectations <file>` writes JSON status baselines and `--expectations <file>` fails on changed/missing/stale statuses. Verified with `cargo check -p serval-wpt`, manifest unit tests, expectation guard unit tests, `serval-wpt list dom/abort` (9 manifest-backed variants) and `serval-wpt list dom/abort --walk-discovery` (10 heuristic files, including `.any.js` misclassified as load). Debug `testharness` still stack-overflows as previously documented; release-mode broad baselines remain the next H3 step.
- 2026-06-29 — **First checked-in WPT guard landed**: generated `ports/serval-wpt/expectations/testharness/dom_abort_boa.json` from release-mode `serval-wpt testharness dom/abort --engine boa`. Baseline is 2 all-pass / 4 with-failures / 0 errored / 0 no-results / 3 skipped, subtests 29/37 passed. Added `support/wpt/check-testharness-baselines.ps1`; verified `powershell -ExecutionPolicy Bypass -File support/wpt/check-testharness-baselines.ps1 -NoBuild` reports `unexpected=0`.
- 2026-06-30 — **H3 completed for the default hostable testharness lane**: added release-mode Boa baselines for full `dom` (`dom_boa.json`), focused `dom/nodes` (`dom_nodes_boa.json`), and `html/webappapis/timers` (`html_webappapis_timers_boa.json`); wired all checked expectations into `support/wpt/check-testharness-baselines.ps1`; added `.github/workflows/wpt-harness.yml` so push/PR CI runs the same guard. Verified `cargo build --release -p serval-wpt` and `powershell -ExecutionPolicy Bypass -File support/wpt/check-testharness-baselines.ps1 -NoBuild` with `unexpected=0`.
- 2026-06-30 — **H2a landed as an explicit Nova snapshot seam**: the direct derive-based structural-clone route was rejected, then replaced with `GcAgent::snapshot_clone`, Nova-side isolation tests, `ScriptEngineSnapshot`, `Runtime::snapshot_clone`, and a `NovaHarnessTemplate` in `serval-wpt` that reuses a post-`testharness.js` Nova heap per test.
- 2026-06-30 — **First release-mode Nova throughput probe on the WPT lane**: `serval-wpt testharness dom --engine nova` still overflows the main-thread stack before a full aggregate, so the broad lane is not yet stable enough to cash in the snapshot-template win. The smaller checked slices do complete: `dom/abort` at exact coarse parity with Boa in 1.38s vs 1.17s, `html/webappapis/timers` at exact coarse parity in 1.48s vs 1.62s, and `dom/nodes` with the same coarse buckets as Boa in 4.44s vs 34.36s.
- 2026-06-30 — **H4a reason metadata landed**: `testharness` expectations can now pin an optional per-test `reason` in addition to coarse status, and the runner emits explicit reasons for capability skips / early error classes. Rewrote the focused checked baselines (`dom_abort_boa.json`, `dom_nodes_boa.json`, `html_webappapis_timers_boa.json`) to exercise the format without re-basing the broad dirty-tree `dom_boa.json`.
- 2026-06-30 — **First checked fetch/server-mode baseline landed locally**: release-mode `serval-wpt --features netfetch testharness fetch/api/basic --spawn-server --engine boa` completed in about 7.9 minutes with 8 all-pass / 8 with-failures / 2 errored / 20 no-results / 0 skipped, subtests 81/119 passed. Checked in `fetch_api_basic_boa.json` plus `support/wpt/check-testharness-fetch-baselines.ps1`. This remains a separate local guard, not default CI, because it depends on local host mapping and live server startup.
- 2026-07-01 — **test262 `async` re-enabled** (serval `493454cdc5c`): the corpus-scale memory blow-up that forced the earlier revert was traced to two *non-async* Promise infinite-hangs spinning in shared threads, not async. Per-test worker-subprocess isolation bounds each async test to its own reaped process, so it cannot recur. Restores `run_262_async` (`$DONE` via a `print`→buffer shim + `run_event_loop` + the `AsyncTestComplete` sentinel) + `Runtime::value_to_string`. `built-ins/Promise/race` skipped 63→0, +6 gaps surfaced; recovers the ~5,582 skipped corpus-wide.
- 2026-07-01 — **Nova Promise iterator-close hang family fixed** (nova fork `e9765334` + `b5201d12`): the combinators used `inner_promise_then` (internal reaction wiring), bypassing the observable `.then` method and never `IteratorClose`-ing, so an infinite iterable with a throwing/overridden `then` looped forever. Fix: `perform_promise_race` creates the capability's shared resolve/reject resolving functions and `Invoke`s the real `.then`; `all`/`allSettled`/`any` (all via `perform_promise_group`) materialize per-element resolve/reject functions — a new optional `group_element` payload on `PromiseResolvingFunctionHeapData` driving the existing `PromiseGroup::settle`, guarded by `[[AlreadyCalled]]` — then `Invoke` `.then` and `IteratorClose` on abrupt (setting `[[Done]]` so the caller does not close twice). Validated in serval: race 2 hangs → pass (nova-only=0); full `built-ins/Promise` (677) both-pass 453→469, boa-only 180→170, **timeout 6→0, nova-only=0** (16 gaps closed: the group/any hangs + observable-`.then` conformance tests, zero regressions). This clears the Promise-combinator half of the stability track; the Nova broad-corpus stack overflow (H2) and the perf cliffs remain.
- 2026-07-01 — **H2 stack-overflow gate cleared** (serval `72b07ecf647`, nova `cce0f09b`): the broad `dom --engine nova` overflow isolated to a re-entrant event-dispatch test whose recursion Nova's fixed 3500-context cap couldn't bound before the OS stack overflowed. Fixed by running the runner on a 512MB stack **and** replacing Nova's count cap with an actual-stack-use guard (stack-pointer proxy, portable to wasm; `stacker` is native-only). Broad `dom --engine nova` now completes in ~12s at 90 all-pass / 2194 subtests — parity with the Boa `dom` baseline. See the H2 "Stack overflow resolved" bullet. Remaining stability item: the perf cliffs (Array/defineProperty `length`, `decodeURI`).
