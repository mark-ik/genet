# Nova conformance campaign

Status: **plan (2026-06-10).** A category-driven program to raise Nova's ECMAScript
conformance, scoped to serval's actual position (the `serval-embedder` fork, not
upstream trynova) and to the two-engine split (Nova native, Boa wasm + oracle). No
code yet; this sets the steering wheel and the order of work.

## Thesis

Conformance is a dashboard-driven campaign by spec category, not a bug-by-bug crawl.
Two facts shape the whole program before any test runs:

1. **serval runs a fork that is already ahead of upstream on RegExp.** Every public
   number (test262.fyi, the trynova README) describes upstream Nova. serval's fork
   swapped the regex engine and closed a string-indexing panic family, so the
   campaign must baseline against the fork's own `expectations.json`, not the
   dashboard.
2. **There are two engines, and they need not both reach 100% on every axis.** Nova is
   native-only; Boa is the wasm backend and the language-axis oracle (~94% test262),
   and it already carries the full ICU4X + Temporal stack. So the language axes split
   between them rather than duplicating.

## The baseline: read the fork, not the dashboard

Upstream rough state (test262.fyi, 2026-06-10): **77.39% overall, 95.81% core
language**, with the holes at **Intl/402 ~0.78%**, **built-ins ~71.26%**, **Annex B
~52.12%**. Test262 covers ECMA-262, ECMA-402, and JSON, and is broad but not complete,
so a high pass rate is necessary, not sufficient, for real-world JS compatibility.

These are **upstream** numbers and understate serval on the regex-touched categories.
The fork already landed (both 2026-06-02):

- **RegExp moved off the `regex` crate onto `regress`** (the ECMAScript backtracking
  engine Boa also uses): **244 improvements on `built-ins/RegExp` + `built-ins/String`,
  0 regressions**, covering lookbehind, named groups, unicodeSets (`v`),
  regexp-modifiers, the `Symbol.replace`/`match`/`split` algorithms, and the
  `d`/`hasIndices` flag. See [the regress doc](./2026-06-02_nova_regress_regex_engine.md).
- **The WTF-8/UTF-16 lone-surrogate panic family closed.** See
  [the indexing-fixes doc](./2026-06-02_nova_wtf8_indexing_fixes.md).

So the upstream blockers "RegExp is non-compliant" and "RegExp needs a compliance push"
are paid down here. The residual regex work is a short tail (sticky-`y` is emulated, a
lone-surrogate *pattern source* is lossy) plus **upstreaming the `regress` swap to
trynova** (the doc flags it an upstream candidate). Do not open a RegExp workstream;
close the tail and upstream the swap.

**Step 0 of the campaign is therefore a fresh fork test262 run** to replace the
upstream `built-ins` / Annex-B numbers with serval's real ones before prioritizing.
The mechanism already exists: Nova's contributing guide tests PRs against
`expectations.json` and gives the commands to regenerate `expectations.json` /
`metrics.json`. The fork's committed `expectations.json` is the steering wheel.

## The two-engine axis split (decide this first)

The campaign's largest lever is not test ordering; it is which engine owns which axis,
given that Nova is native-only and Boa is wasm + oracle and already has ICU4X.

- **Boa owns the Intl/ECMA-402 and Temporal axis.** Boa's dependency graph already
  pulls the full stack (`icu_collections`, `icu_locale`, `icu_properties`,
  `icu_normalizer`, `icu_calendar`, `timezone_provider`, `temporal_rs`). Nova's 402
  pass rate is ~0.78%. Hand-rolling 402 on Nova duplicates a subsystem Boa already
  carries.
- **Nova owns ECMA-262 core (already ~96%), the built-ins clusters, the regex tail,
  and realms/host-hooks.** These are the native-hot-path obligations for content
  actors, where Nova is the primary engine.

This reframes the single biggest item below (Intl) from "a subsystem to build on Nova"
into "an axis Boa already covers," and focuses Nova's effort where it is the load-bearing
engine.

## The work program

Reordered from the original seven so architectural clusters precede leaves, and scoped
per the split above.

1. **Make test262 the steering wheel, by category.** Run the fork tree, diff against
   the committed `expectations.json`, and drive the campaign off the category
   dashboard (`metrics.json`), not bug-by-bug. This is the contributing-guide workflow
   the fork already expects.
2. **Fix architectural blockers before leaf failures.** Of the upstream README's named
   gaps, RegExp is **done in-fork** (above). The ones that stand and cut across many
   tests: **sparse arrays mis-allocating for huge sparse lengths**, **Promise
   subclassing missing** (the `PromiseCapability` source comments it explicitly;
   distinct from the host-promise/deferred primitive added 2026-06-10, which does not
   touch subclassing), and **WebAssembly execution absent** (a fullweb-tier concern,
   defer). Prioritize sparse arrays and Promise subclassing.
3. **Finish built-ins by spec cluster, not by easiest test.** Target whole clusters
   that remove classes of expected failures: `Temporal`, `Promise` (incl. subclassing),
   `Iterator`, `Set`, `TypedArray`, `DataView`, `Atomics`, and module/import behavior.
   Chase clusters, not low-hanging leaves.
4. **Realms and host hooks.** The contributing doc calls realm-specific heap work an
   active design area. For content actors, cross-realm objects, modules, jobs/microtasks,
   and host hooks are what turn "looks like JS" into "acts like JS." serval already
   exercises this seam (the `HostHooks` job queue + the host-promise primitive), so it
   is partly load-bearing already.
5. **Close the RegExp tail and upstream the swap.** Sticky-`y` native support,
   lone-surrogate pattern source, then push `regress` to trynova so the fork stops
   carrying it.
6. **Differential fuzzing, once the dashboard climbs.** Compare Nova against
   V8/SpiderMonkey/JSC (and against Boa, the in-tree oracle) on generated programs.
   test262 catches spec obligations; fuzzing catches interactions between them.

Intl/402 is deliberately absent from Nova's list: it rides Boa per the split.

## Scope: profile tier and Intl deferral

"Total conformance including ECMA-402" is the wrong near-term target for the campaign
that is actually in flight. serval's profile ladder (static / interactive / scripted /
fullweb) puts the live work at the **scripted tier** (testharness + DOM), which needs no
Intl. Full 402 is a **fullweb-tier** obligation, and on that tier Boa likely carries it
anyway. Scope Nova's campaign to ECMA-262; let 402 lag to fullweb.

## Relationship to existing docs

- [Script engine plan](./2026-05-20_serval_script_engine_plan.md) owns the
  `ScriptEngine` trait and the engine ladder; this doc is the conformance program that
  rides on top.
- [Pluggable engines / testharness plan](./2026-05-26_pluggable_engines_testharness_plan.md)
  established the two-axis (Nova vs Boa) runner that makes a category dashboard per
  engine observable. That runner is how step 1 is measured.
- [Regress regex doc](./2026-06-02_nova_regress_regex_engine.md) and
  [WTF-8 indexing doc](./2026-06-02_nova_wtf8_indexing_fixes.md) are the already-landed
  regex work this plan treats as done.
- [JS execution strategy](./2026-05-25_js_execution_strategy.md) is the orthogonal
  speed axis (interpret/JIT/AOT/weval); conformance and speed are separate campaigns.

## Sources

- Upstream conformance dashboard: [test262.fyi](https://test262.fyi/),
  [TC39 test262](https://github.com/tc39/test262) (covers ECMA-262/402/JSON; broad, not
  complete).
- Nova upstream: [CONTRIBUTING](https://github.com/trynova/nova/blob/main/CONTRIBUTING.md)
  (expectations.json / metrics.json workflow), [README](https://github.com/trynova/nova)
  (named gaps: sparse arrays, RegExp, Promise subclassing, Wasm).
- serval fork state: the regress and WTF-8 docs above; Boa's ICU4X/Temporal stack
  observed in the `script-engine-boa` build graph.
