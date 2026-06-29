# WPT harness exactness + throughput plan

**Date:** 2026-06-24
**Status:** in progress — H1 (manifest reader), H2b (cross-engine compare), and **H5 (test262 runner + hang-safe full-corpus run)** landed; **H2a (Nova `GcAgent::clone` snapshot) deferred** as measured-unnecessary for a corpus-safe run (parallel worker subprocesses suffice); H3/H4 open. Spun out of the grand audit (`2026-06-24_grand_audit.md` §2, levers 1/3/5); continues the WPT runner plan (`2026-05-26_wpt_runner_plan.md`, whose Discovery section already flags "no MANIFEST.json yet").
**Thesis:** the binding constraint on serval's WPT scoreboard is the harness, not the engine. What runs and how much runs gates the value of every engine fix. This plan closes the three harness levers in dependency order: exactness (what runs), throughput (how much runs), then a tracked scoreboard + regression guard (so movement is real and stays).

## Why this and not engine work first

The audit re-measured the engine far ahead of its stale reputation (DOM core panic-free on both engines; CSS reftests 5-40x the circulated baselines). The remaining waste is that the runner discovers and scores the wrong set of tests, re-pays the testharness.js eval per test, and has no checked-in expectations, so nobody can trust a delta. Three harness levers fix that and are precondition for steering the CSS/DOM levers.

## Phases (done-conditions, not dates)

### H1 — MANIFEST.json reader (lever 1)

Replace the ad-hoc directory walk and heuristic expansion with the upstream-generated manifest.
- Today: `ports/serval-wpt/src/main.rs:174` collects via a raw walk; `:211` `synthesize_any_js` hand-expands `.any.*`; `:719` `parse_fuzzy` reconstructs fuzzy metadata; no `MANIFEST` reference exists in `src/`.
- Build: run `wpt manifest` once into the checked-out tree, parse the generated JSON, and drive test classification, variant (`?query`) expansion, `.any.js` -> `.any.html`/`.any.worker.html` multi-global enumeration, per-test timeouts, expected-reference resolution, and fuzzy metadata from it.
- **Done when** the runner enumerates and classifies tests from MANIFEST.json (the heuristic walk and `synthesize_any_js`/`parse_fuzzy` paths are deleted or demoted to a fallback), and a spot-check directory's runnable-test count matches `wpt run`'s enumeration.

### H2 — Snapshot-clone Runtime pool (lever 3)

Amortize the dominant per-test cost.
- Today: each test builds a fresh `Runtime` and re-evals the 5,207-line testharness.js. The bench probe (`harness.rs:393-414`) proves the eval, not `Runtime::new()`, is the dominant cost, that naive Runtime reuse leaks the `tests` singleton across re-evals, and prescribes a post-(harness-eval) snapshot cloned per test via the `GcAgent::clone` path.
- Build: eval testharness.js once into a base agent, then `GcAgent::clone` a fresh per-test agent from that snapshot so each test starts post-harness-eval with a clean `tests` singleton.
- **Engine target (corrected 2026-06-25, grounded):** the runner scores on **Boa** by default (`main.rs:297`), but `GcAgent` is **Nova-only and has no `clone`/`snapshot`** (only `new`), and Boa's `Context` has no clone either — the prescription is mismatched *and* unbuilt. Per the conformance-target doctrine (improve **Nova**, keep **Boa** pristine as the oracle), the snapshot belongs in **Nova**: build `GcAgent::clone` there for fast routine Nova-scored runs; Boa stays slow-but-pristine, run as the reference. Do **not** add a snapshot to Boa. The snapshot is an *optional per-engine capability* behind the `ScriptEngine` trait (a future V8 / SpiderMonkey / QuickJS brings its own, or none), so the harness must not assume any engine can clone.
- **Done when** a full dom/ subset run on Nova shows the per-test cost dominated by the test body, not the harness eval, and the `tests`-singleton leak is gone (re-runs are deterministic).

### H2b — Per-engine scoring + cross-engine diff (the Nova-improvement driver)

`run_test` is already generic over `E: ScriptEngine`, so scoring both engines on one corpus and diffing is a small addition with outsized value. A test that **passes on Boa but fails on Nova is a Nova JS-engine gap** (a fork improvement; watching this bucket shrink is the Nova-to-Boa gap closing). A test that **fails on both is a serval-platform gap** (layout / DOM, not the engine). This converts the scoreboard into a per-test worklist routed to the right owner, and operationalizes the audit's keep-Nova-80-and-Boa-94-distinct: Nova is the primary "are we improving" number, Boa the platform ceiling, `Boa − Nova` the Nova fork's remaining JS work. The two engines also map to the two PWA lanes (Nova/wasm64 on Chrome/Firefox; Boa/wasm32 on WebKit). Buildable on subsets today (no snapshot needed); H2a's snapshot is what makes it affordable on the full corpus.

- **Done when** a `compare <subset>` run reports the 2x2 (both-pass / both-fail / Boa-only / Nova-only) and the Boa-only set is surfaced as Nova's worklist.
- **Empirical (2026-06-25, `compare` landed):** across `dom/abort`, `dom/nodes` (302 tests), and `html/webappapis/timers`, **Boa and Nova are at exact parity — zero Boa-only (Nova) gaps**; every failure fails on *both* engines (e.g. dom/nodes 56 both-pass / 215 both-fail). So on WPT, **the failures are serval-platform** (DOM / layout / parsing — the audit's object-fit / interface-table / CSS levers), not the JS engine, and improving Nova moves nothing here. The Nova-vs-Boa gap is **ECMAScript language conformance (test262, which WPT excludes by design)**, so the **Nova worklist comes from a test262 runner scoring Nova, not from WPT-`compare`**. WPT-`compare`'s standing role is therefore **regression detection** (catch a future Nova divergence from the Boa oracle) and parity confirmation, not the Nova worklist. The manifest already carries a `test262` item type; a test262-`compare` (different harness: `$262` + frontmatter, not testharness.js) is the lever for Nova's actual gaps.

### H3 — Corpora re-score + checked-in expectations + regression guard (lever 5)

Turn measurement into a guardrail.
- Re-run dom/ and fetch/ (and the CSS subsets the conformance plan tracks) on H1+H2, and publish current aggregates. The audit found several levers sized against numbers that no longer exist (floats 7 vs 42, css-backgrounds 15 vs 334, normal-flow 1 vs 462; css-multicol claimed 0 but 103/923; css-writing-modes claimed zeroed but 219/1829).
- Add a checked-in expectations file (per-test expected status) and a script that diffs a run against it, so a regression is a failed check rather than an unnoticed count drop.
- **Done when** a tracked aggregate exists per measured directory, and a CI/local check fails on regressions against the checked-in expectations (this is the difference between a measurement tool and a guardrail).

## Sequencing

H1 -> H2 -> H3. H1 makes the right set runnable; H2 makes running the full corpora cheap enough to do routinely; H3 only becomes trustworthy and repeatable once both land. H3's expectations file pairs with the serval-CI sidequest (capability-activation plan) so the guard runs automatically.

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

- 2026-06-24 (from the grand audit, adversarially verified): the runner is 2,770 LOC (`main.rs` 1,671); no MANIFEST reader; `harness.rs` bench prescribes but does not implement the snapshot-clone pool. fetch/ runs only behind an off-by-default feature + a manual hosts-file edit; XHTML/.xht files are skipped (`main.rs:587-596`). CSP, websockets/, and h3 are unrunnable through the runner despite netfetcher shipping the transports.
- The "re-score floats/normal-flow/css-backgrounds" sub-lever is already largely done inside the CSS conformance doc's scoreboard; H3's residual value is fresh dom/fetch aggregates + the expectations guard, not re-scoring CSS from scratch.

## Progress

- 2026-06-24 — Plan created from the grand audit. No code yet. H1 is the entry point.
- 2026-06-25 — **H1 reader + `manifest` command landed** (serval `a9703342ecd`): a MANIFEST.json reader (`ports/serval-wpt/src/manifest.rs` — URLs / kind / refs / fuzzy / pre-expanded variants; unit-tested + integration-tested against the real ~39MB manifest) and `serval-wpt manifest <subset>`. **Validated vs the walk on `dom/nodes`:** manifest 319 runnable (testharness 302, reftest 3, crashtest 14) vs walk 342 — the walk over-counts (38 `load` + 2 `reference` non-tests) and under-counts variants (+17 testharness), confirming the heuristic enumeration scores the wrong set. Additive (the run path still walks; slice 3 wires the manifest through it).
- 2026-06-25 — **H2 corrected** (above): the snapshot goes in **Nova**, not Boa (Boa is the pristine oracle); added **H2b** (per-engine scoring + cross-engine diff) as the Nova-improvement driver.
- 2026-06-25 — **H2b `compare` landed** (serval `c27d98d4145`): runs each testharness test on Boa + Nova and routes failures (both-fail = serval-platform, Boa-only = Nova gap). **Finding:** Boa/Nova at exact parity on `dom/abort`, `dom/nodes`, `html/webappapis/timers` (0 Nova gaps); WPT failures are serval-platform, so the Nova worklist is a **test262** matter, and WPT-`compare`'s role is regression-detection. Gotcha: run the runner in **release** (debug frames overflow the stack on bounded-deep recursion; the audit's "panic-free on both engines" holds in release).
- 2026-06-28 — **H5 test262 runner landed**: core + run-path + cross-engine `test262 <subset>` (`5df84ab9e23`, `d133f56350f`), confirming the H2b finding empirically (`built-ins/Temporal/Now` 66/66 boa-only — Nova lacks Temporal; `optional-chaining` at parity). Parallelized across cores (shared work-index, `8a5a393b1bb`); **measured ~3.2x, not the hoped ~14x** — jemalloc (`servo-allocator`) is already linked, so the ceiling is memory-bandwidth + per-test agent churn, not the allocator. Module support via `eval_module` (`d149fc649e5`); negative **error-type** matching via `ScriptEngine::describe_error` (`016dffea9fd`); `--worklist-out` full dump (`1b5feddb8d8`).
- 2026-06-28 — **hang-safe runner** (`90ae2edc268`, `aefc0db6103`, `f690f364f22`): the engines can't be step-metered (`eval_bounded` is unbounded for Boa+Nova) and a *non-async* `Promise.race` iterator-close test hangs serval, so a corpus run needs **per-test worker-subprocess isolation** + kill-on-`--timeout` (default 30s), not an in-process watchdog. Each timeout is attributed to the engine that never reported. This subsumes the parallelism and **defers H2a's `GcAgent::clone`** (measured unnecessary for corpus-safety). `async` was built + validated per-test but reverted (corpus-scale memory accumulation; pending investigation).
- 2026-06-29 — **full corpus run** (53,166 tests): `both-pass=35858 both-fail=3374 boa-only=7818 (Nova gap) nova-only=513 timeout=21 skipped=5582` → Nova 76.5% / Boa 91.8%. **The Nova worklist is 75% Temporal** (5,873 of 7,818); next tiers RegExp/Iterator/Set/Promise + a 533-test language-core tail; see the H5 result section. Found a systematic Nova Promise-combinator iterator-close **hang family** (6 tests) and perf cliffs on both engines (Nova: Array/defineProperty `length`, `decodeURI`; Boa: `Date/dst-offset-caching`).
