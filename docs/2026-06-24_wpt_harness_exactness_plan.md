# WPT harness exactness + throughput plan

**Date:** 2026-06-24
**Status:** in progress — H1 is wired into the normal runner path (manifest-backed discovery by default; legacy walk demoted to `--walk-discovery`), H2b (cross-engine compare), **H3 (checked-in testharness expectations + local/CI guard)**, and **H5 (test262 runner + hang-safe full-corpus run)** landed; **H2a (Nova `GcAgent::clone` snapshot) is deferred and now scoped as a Nova VM snapshot API project, not a harness-only patch**. H4's policy metadata remains open. Spun out of the grand audit (`2026-06-24_grand_audit.md` §2, levers 1/3/5); continues the WPT runner plan (`2026-05-26_wpt_runner_plan.md`, whose Discovery section already flags "no MANIFEST.json yet").
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
- Today: each test builds a fresh `Runtime` and re-evals the 5,207-line testharness.js. The bench probe (`harness.rs:393-414`) proves the eval, not `Runtime::new()`, is the dominant cost, that naive Runtime reuse leaks the `tests` singleton across re-evals, and prescribes a post-(harness-eval) snapshot cloned per test via the `GcAgent::clone` path.
- Build: eval testharness.js once into a base agent, then `GcAgent::clone` a fresh per-test agent from that snapshot so each test starts post-harness-eval with a clean `tests` singleton.
- **Engine target (corrected 2026-06-25, grounded):** the runner scores on **Boa** by default (`main.rs:297`), but `GcAgent` is **Nova-only and has no `clone`/`snapshot`** (only `new`), and Boa's `Context` has no clone either — the prescription is mismatched *and* unbuilt. Per the conformance-target doctrine (improve **Nova**, keep **Boa** pristine as the oracle), the snapshot belongs in **Nova**: build `GcAgent::clone` there for fast routine Nova-scored runs; Boa stays slow-but-pristine, run as the reference. Do **not** add a snapshot to Boa. The snapshot is an *optional per-engine capability* behind the `ScriptEngine` trait (a future V8 / SpiderMonkey / QuickJS brings its own, or none), so the harness must not assume any engine can clone.
- **2026-06-30 implementation spike:** a direct derive-based `GcAgent`/`Agent`/`Heap` clone probe in the Nova fork does **not** compile and should not be pursued as a mechanical patch. The blockers are structural:
  - `Heap` contains non-clone `SoAVec` arenas (`arrays`, `maps`, `sets`, `finalization_registrys`) and `soavec 0.2.0` does not implement `Clone` for `SoAVec`.
  - Core heap records lack `Clone`, including `ArrayBufferHeapData`, `ECMAScriptFunctionHeapData`, `ElementArrays`, `Environments`, `ObjectRecord`, `RegExpHeapData`, `ScriptRecord`, `SourceCodeHeapData`, `SourceTextModuleRecord`, and module `LoadedModules`.
  - A raw structural clone would be unsound even if it compiled: Serval's Nova host hooks carry the microtask job queue and module cache, while the realm `[[HostDefined]]` slot carries DOM host data, reflector roots, pending host promises, and the release queue. A snapshot clone must replace those with fresh per-clone host state.
- **Required Nova primitive:** add an explicit snapshot API, not `#[derive(Clone)]`: `GcAgent::snapshot_clone(new_host_hooks, host_defined_policy)` or equivalent. It must assert no JS is running, clone only VM state that is safe to duplicate, reset transient stacks, install replacement host hooks, clear or replace realm `[[HostDefined]]`, and have Nova-side tests proving original/clone isolation. Only after that lands should `script-engine-api` grow an optional `ScriptEngineSnapshot` capability and the WPT harness use it for Nova.
- **Done when** a full dom/ subset run on Nova shows the per-test cost dominated by the test body, not the harness eval, and the `tests`-singleton leak is gone (re-runs are deterministic).

### H2b — Per-engine scoring + cross-engine diff (the Nova-improvement driver)

`run_test` is already generic over `E: ScriptEngine`, so scoring both engines on one corpus and diffing is a small addition with outsized value. A test that **passes on Boa but fails on Nova is a Nova JS-engine gap** (a fork improvement; watching this bucket shrink is the Nova-to-Boa gap closing). A test that **fails on both is a serval-platform gap** (layout / DOM, not the engine). This converts the scoreboard into a per-test worklist routed to the right owner, and operationalizes the audit's keep-Nova-80-and-Boa-94-distinct: Nova is the primary "are we improving" number, Boa the platform ceiling, `Boa − Nova` the Nova fork's remaining JS work. The two engines also map to the two PWA lanes (Nova/wasm64 on Chrome/Firefox; Boa/wasm32 on WebKit). Buildable on subsets today (no snapshot needed); H2a's snapshot is what makes it affordable on the full corpus.

- **Done when** a `compare <subset>` run reports the 2x2 (both-pass / both-fail / Boa-only / Nova-only) and the Boa-only set is surfaced as Nova's worklist.
- **Empirical (2026-06-25, `compare` landed):** across `dom/abort`, `dom/nodes` (302 tests), and `html/webappapis/timers`, **Boa and Nova are at exact parity — zero Boa-only (Nova) gaps**; every failure fails on *both* engines (e.g. dom/nodes 56 both-pass / 215 both-fail). So on WPT, **the failures are serval-platform** (DOM / layout / parsing — the audit's object-fit / interface-table / CSS levers), not the JS engine, and improving Nova moves nothing here. The Nova-vs-Boa gap is **ECMAScript language conformance (test262, which WPT excludes by design)**, so the **Nova worklist comes from a test262 runner scoring Nova, not from WPT-`compare`**. WPT-`compare`'s standing role is therefore **regression detection** (catch a future Nova divergence from the Boa oracle) and parity confirmation, not the Nova worklist. The manifest already carries a `test262` item type; a test262-`compare` (different harness: `$262` + frontmatter, not testharness.js) is the lever for Nova's actual gaps.

### H3 — Corpora re-score + checked-in expectations + regression guard (lever 5)

Turn measurement into a guardrail.
- Re-run hostable testharness subsets on H1 and publish current aggregates. Fetch remains a server-mode/netfetch lane because it needs `wpt serve`, the `netfetch` feature, and local host mapping; it is not part of the default disk-mode guard. The CSS re-score is already carried by the CSS conformance scoreboard.
- Landed mechanism: `testharness` accepts `--write-expectations <file>` and `--expectations <file>`. The JSON format is per-test URL -> coarse status (`pass`, `fail`, `error`, `no-results`, `skip`); the check fails on changed status, missing expectation, or stale expectation so `unexpected=0` is enforceable.
- Checked baselines:
  - `ports/serval-wpt/expectations/testharness/dom_boa.json` pins the full hostable `dom` manifest subset on Boa in release mode: 88 all-pass, 365 with-failures, 72 errored, 84 no-results, 51 skipped; subtests 2122/6671 passed.
  - `ports/serval-wpt/expectations/testharness/dom_abort_boa.json` keeps the focused abort slice pinned: 2 all-pass, 4 with-failures, 0 errored, 0 no-results, 3 skipped; subtests 29/37 passed.
  - `ports/serval-wpt/expectations/testharness/dom_nodes_boa.json` keeps the focused DOM-nodes slice pinned: 57 all-pass, 205 with-failures, 12 errored, 14 no-results, 42 skipped; subtests 1655/5365 passed.
  - `ports/serval-wpt/expectations/testharness/html_webappapis_timers_boa.json` pins the timer/event-loop smoke slice: 4 all-pass, 0 with-failures, 8 errored, 0 no-results, 0 skipped; subtests 7/7 passed.
- Local guard: `support/wpt/check-testharness-baselines.ps1` builds `serval-wpt` release and checks all listed baselines; `-NoBuild` reuses an existing release binary.
- CI guard: `.github/workflows/wpt-harness.yml` runs the same guard on push and pull request.
- **Done condition: met for the default hostable testharness lane.** A local or CI run now fails on changed, missing, or stale expectations.

## Sequencing

The practical sequence is now H1 -> H3 guard -> H4 policy metadata, with H2a left as an optional Nova optimization. H5's subprocess runner already provides the affordability/safety lever for test262. Fetch coverage is a separate server-mode/netfetch guard because the default disk-mode harness cannot own the required WPT server + host mapping setup.

## Non-goals

- Engine fixes (owned by the CSS conformance + HTML interface-table plans).
- A full `wpt serve` orchestration rewrite; the live-server fetch slice already works in server mode.
- iframe/second-realm execution (a larger harness capability; note it as a known wall, do not scope it here).

## H4 — Governance: green-by-default, with sub-WPT micro-tests (from the formal-web harvest)

The gterzian/formal-web harvest (`2026-06-24_formal_web_lessons.md`) supplies a
governance model worth adopting alongside H3, turning the runner from a noisy
dashboard into a regression gate:

- **Skip-by-default `include.ini` + per-file opt-in**, and **`meta/*.ini`
  expected-result files** pinning expected pass/fail per sub-test with TODO
  reasons, so the default run asserts **`unexpected = 0`**. A new pass becomes an
  explicit metadata edit, not an invisible count drift. This is the policy layer
  over H3's aggregates + expectations guard.
- **Local deterministic micro-tests below the WPT level.** Small `.html` tests
  reporting via testharness.js OR a plain `window.__formalWebTestResult` object
  (same shape testharnessreport produces), mounted so they reuse upstream
  `/resources/testharness.js`. These let serval lock an event-loop / parser /
  streams milestone *before* the corresponding WPT directory is enabled — which
  matters now, while whole directories are gated by the H1/H2 work. (The
  `byob-debug.html`-style micro-test is the model; see the BYOB streams plan.)

**Optional rigor capability — TLA+ trace validation of the scheduler.** Distinct
from WPT, and architecture-agnostic (it needs only an event log + a model).
serval's single-process model makes it *easier* than formal-web's (one in-process
channel + a single counter clock, no cross-process monitor or channel-closure
dance). Tap the five named task boundaries (`script-runtime-api/lib.rs:266`/`:309`,
`dispatch_event`, `eval`, `pump_microtasks`) to emit NDJSON, write one base+trace
TLA+ spec pair for one protocol, and run TLC in CI (the Cirstea/Kuppe method).
This is a months-shaped investment; tracked as the rigor arm of serval's
`2026-06-24_event_loop_rigor_plan.md`, not required for the harness levers here.

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
  subsumes the earlier in-process thread-scope) and the affordability lever, so **H2a's
  `GcAgent::clone` is deferred** — not needed for a corpus-safe run, and fork-risky. (jemalloc
  is already linked; per-test cost is engine-bound, ~0.1s subprocess startup is the price.)
- **`async` is deferred:** built and validated per-test (correct, finds gaps), but at corpus
  scale the async event-loop path accumulated memory; reverted pending investigation. The
  ~5,582 skipped are mostly async + missing-includes.
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

## Findings

- Historical 2026-06-24 finding (from the grand audit, adversarially verified at the time): the runner was 2,770 LOC (`main.rs` 1,671), had no MANIFEST reader in the run path, and had no checked-in expectations guard. This is now stale for the default hostable testharness lane: manifest discovery is wired into normal commands, JSON expectations exist, and the checked baselines run under a local/CI guard.
- `harness.rs` bench still prescribes but does not implement the snapshot-clone pool; H2a remains deferred because test262 corpus-safety came from subprocess isolation instead. A 2026-06-30 Nova compile probe showed the first real dependency is a Nova-owned snapshot API with explicit arena/module/host-state semantics.
- fetch/ runs only behind an off-by-default feature + a manual hosts-file edit. XHTML/.xht files are skipped. CSP, websockets/, and h3 are unrunnable through the runner despite netfetcher shipping the transports.
- The "re-score floats/normal-flow/css-backgrounds" sub-lever is already largely done inside the CSS conformance doc's scoreboard; H3's residual value is fresh dom/fetch aggregates + the expectations guard, not re-scoring CSS from scratch.

## Progress

- 2026-06-24 — Plan created from the grand audit. No code yet. H1 is the entry point.
- 2026-06-25 — **H1 reader + `manifest` command landed** (serval `a9703342ecd`): a MANIFEST.json reader (`ports/serval-wpt/src/manifest.rs` — URLs / kind / refs / fuzzy / pre-expanded variants; unit-tested + integration-tested against the real ~39MB manifest) and `serval-wpt manifest <subset>`. **Validated vs the walk on `dom/nodes`:** manifest 319 runnable (testharness 302, reftest 3, crashtest 14) vs walk 342 — the walk over-counts (38 `load` + 2 `reference` non-tests) and under-counts variants (+17 testharness), confirming the heuristic enumeration scores the wrong set. Additive (the run path still walks; slice 3 wires the manifest through it).
- 2026-06-25 — **H2 corrected** (above): the snapshot goes in **Nova**, not Boa (Boa is the pristine oracle); added **H2b** (per-engine scoring + cross-engine diff) as the Nova-improvement driver.
- 2026-06-25 — **H2b `compare` landed** (serval `c27d98d4145`): runs each testharness test on Boa + Nova and routes failures (both-fail = serval-platform, Boa-only = Nova gap). **Finding:** Boa/Nova at exact parity on `dom/abort`, `dom/nodes`, `html/webappapis/timers` (0 Nova gaps); WPT failures are serval-platform, so the Nova worklist is a **test262** matter, and WPT-`compare`'s role is regression-detection. Gotcha: run the runner in **release** (debug frames overflow the stack on bounded-deep recursion; the audit's "panic-free on both engines" holds in release).
- 2026-06-28 — **H5 test262 runner landed**: core + run-path + cross-engine `test262 <subset>` (`5df84ab9e23`, `d133f56350f`), confirming the H2b finding empirically (`built-ins/Temporal/Now` 66/66 boa-only — Nova lacks Temporal; `optional-chaining` at parity). Parallelized across cores (shared work-index, `8a5a393b1bb`); **measured ~3.2x, not the hoped ~14x** — jemalloc (`servo-allocator`) is already linked, so the ceiling is memory-bandwidth + per-test agent churn, not the allocator. Module support via `eval_module` (`d149fc649e5`); negative **error-type** matching via `ScriptEngine::describe_error` (`016dffea9fd`); `--worklist-out` full dump (`1b5feddb8d8`).
- 2026-06-28 — **hang-safe runner** (`90ae2edc268`, `aefc0db6103`, `f690f364f22`): the engines can't be step-metered (`eval_bounded` is unbounded for Boa+Nova) and a *non-async* `Promise.race` iterator-close test hangs serval, so a corpus run needs **per-test worker-subprocess isolation** + kill-on-`--timeout` (default 30s), not an in-process watchdog. Each timeout is attributed to the engine that never reported. This subsumes the parallelism and **defers H2a's `GcAgent::clone`** (measured unnecessary for corpus-safety). `async` was built + validated per-test but reverted (corpus-scale memory accumulation; pending investigation).
- 2026-06-29 — **full corpus run** (53,166 tests): `both-pass=35858 both-fail=3374 boa-only=7818 (Nova gap) nova-only=513 timeout=21 skipped=5582` → Nova 76.5% / Boa 91.8%. **The Nova worklist is 75% Temporal** (5,873 of 7,818); next tiers RegExp/Iterator/Set/Promise + a 533-test language-core tail; see the H5 result section. Found a systematic Nova Promise-combinator iterator-close **hang family** (6 tests) and perf cliffs on both engines (Nova: Array/defineProperty `length`, `decodeURI`; Boa: `Date/dst-offset-caching`).
- 2026-06-29 — **H1 normal-runner wiring + H3a expectation mechanism landed**: `list`, `run`, `reftest`, `dump`, `testharness`, and `compare` now discover from MANIFEST.json by default; `--walk-discovery` keeps the old heuristic path as a diagnostic fallback. Manifest-selected generated `.any.html` variants retain their WPT URL while resolving scripts from the backing `.any.js` file. Reftest refs/fuzzy come from the manifest with HTML parsing as fallback. `testharness --write-expectations <file>` writes JSON status baselines and `--expectations <file>` fails on changed/missing/stale statuses. Verified with `cargo check -p serval-wpt`, manifest unit tests, expectation guard unit tests, `serval-wpt list dom/abort` (9 manifest-backed variants) and `serval-wpt list dom/abort --walk-discovery` (10 heuristic files, including `.any.js` misclassified as load). Debug `testharness` still stack-overflows as previously documented; release-mode broad baselines remain the next H3 step.
- 2026-06-29 — **First checked-in WPT guard landed**: generated `ports/serval-wpt/expectations/testharness/dom_abort_boa.json` from release-mode `serval-wpt testharness dom/abort --engine boa`. Baseline is 2 all-pass / 4 with-failures / 0 errored / 0 no-results / 3 skipped, subtests 29/37 passed. Added `support/wpt/check-testharness-baselines.ps1`; verified `powershell -ExecutionPolicy Bypass -File support/wpt/check-testharness-baselines.ps1 -NoBuild` reports `unexpected=0`.
- 2026-06-30 — **H3 completed for the default hostable testharness lane**: added release-mode Boa baselines for full `dom` (`dom_boa.json`), focused `dom/nodes` (`dom_nodes_boa.json`), and `html/webappapis/timers` (`html_webappapis_timers_boa.json`); wired all checked expectations into `support/wpt/check-testharness-baselines.ps1`; added `.github/workflows/wpt-harness.yml` so push/PR CI runs the same guard. Verified `cargo build --release -p serval-wpt` and `powershell -ExecutionPolicy Bypass -File support/wpt/check-testharness-baselines.ps1 -NoBuild` with `unexpected=0`.
- 2026-06-30 — **H2a scoped, not landed**: tried the direct Nova structural-clone route (`GcAgent`/`Agent`/`Heap` derives) and rejected it after `cargo check -p nova_vm` exposed non-clone `SoAVec` arenas and non-clone heap/module records. Reverted the probe; Nova remains clean and `cargo check -p nova_vm` passes. The required next unit is an explicit Nova snapshot API that replaces host hooks and realm host-defined data, followed by a Serval optional `ScriptEngineSnapshot` seam.
