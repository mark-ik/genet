# Nova conformance campaign

Status: **plan (2026-06-10).** A category-driven program to raise Nova's ECMAScript
conformance, scoped to genet's actual position (the `genet-embedder` fork, not
upstream trynova) and to the two-engine split (Nova native, Boa wasm + oracle). No
code yet; this sets the steering wheel and the order of work.

## Thesis

Conformance is a dashboard-driven campaign by spec category, not a bug-by-bug crawl.
Two facts shape the whole program before any test runs:

1. **genet runs a fork that is already ahead of upstream on RegExp.** Every public
   number (test262.fyi, the trynova README) describes upstream Nova. genet's fork
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

These are **upstream** numbers and understate genet on the regex-touched categories.
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
upstream `built-ins` / Annex-B numbers with genet's real ones before prioritizing.
The mechanism already exists: Nova's contributing guide tests PRs against
`expectations.json` and gives the commands to regenerate `expectations.json` /
`metrics.json`. The fork's committed `expectations.json` is the steering wheel.

### Step 0 result (recorded 2026-06-11)

Derived from the committed `expectations.json` + `metrics.json`, which agree exactly
(6,785 fail / 52 crash / 18 timeout / 37 unresolved) and reflect a state `nova_vm` has
not changed since, so a fresh run reproduces it — no 30-minute run needed.

**Overall: 40,515 / 50,733 pass (~80%)**, ahead of upstream's 77.39%. Non-pass by area:

| Area | Non-pass | Detail |
| --- | --- | --- |
| built-ins | 5,491 | **Temporal 3,671** (54% of all failures), RegExp 320, Iterator 224, Promise 193, Set 151, Array 115, resource-mgmt (DisposableStack + Async + Suppressed) ~165, ShadowRealm 64 |
| language | 888 | core syntax / semantics |
| staging | 510 | unstable proposals, low priority |
| **intl402** | **0 (skipped)** | the ~3,326 skips are the Intl tree; Nova does not attempt 402 |

Two masses dominate, and both are **shared-upstream-crate** stories, not hand-rolling:

- **Temporal (3,671, 54% of failures): Nova already binds `temporal_rs`**
  (`nova_vm` `temporal` feature, on by default) — the *same* crate Boa binds. So this is
  **incomplete binding, not a missing library**; closing it is finishing the
  `temporal_rs` wiring, which is upstream trynova's active work. The fork rides it by
  tracking upstream, exactly as it did the `regress` swap. (This corrects the "Boa owns
  Temporal" framing below: both engines wrap one `temporal_rs`.) Measured: the fork
  passes **586 / 4,257 Temporal tests (~14%)** — early, not half-built. Upstream binds it
  method-by-method (foundation `Instant` / `Duration` Feb 2026, `PlainTime` methods
  through May), so the remaining types (`PlainDate`, `PlainDateTime`, `ZonedDateTime`,
  calendars — most of the 4,257) are a multi-quarter upstream grind. The fork (pinned
  2026-06-02) is roughly current with that, so there is **no rebase windfall today**; the
  54% mass clears gradually as upstream lands more. Ride it, do not duplicate it.
  *(2026-07-09: "no windfall today" is a 2026-06-11 reading with the fork pinned
  2026-06-02. Upstream binds `temporal_rs` method-by-method and a month-plus has since
  accrued, so re-measure the rebase delta before opening any Temporal work and harvest
  what upstream landed rather than hand-binding in-fork. See the rebase precondition in
  the work program below.)*
- **Intl/402 (skipped): Nova does *not* bind ICU4X** (no `icu_*` in `nova_vm`). This is
  the one Boa genuinely has and Nova lacks. Closing it means adding an ICU4X binding to
  Nova (a larger, fullweb-tier lift), not copying Boa.

**Strip those two and Nova is ~93% on the ECMA-262 surface it targets** (40,515 /
~43,700). The genuine fork-local residual is ~3,100 mid-size built-in clusters
(Iterator 224, Promise 193 incl. subclassing, Set 151, the RegExp tail 320,
resource-management ~165, ShadowRealm 64) plus 888 language — each a clean class of
failures, none urgent, most at least partly upstream.

## The two-engine axis split (decide this first)

The campaign's largest lever is not test ordering; it is which engine owns which axis,
given that Nova is native-only and Boa is wasm + oracle and already has ICU4X.

- **Shared upstream crates carry Intl + Temporal, not Boa itself.** Both engines bind
  the same neutral libraries: `temporal_rs` for Temporal, ICU4X (`icu_*`) for Intl. Nova
  **already binds `temporal_rs`** (default feature), so its 3,671 Temporal failures are
  incomplete binding, not a missing subsystem, and finishing them is upstream trynova's
  work, not a fork duplication (see the Step 0 result above). Intl is the genuine gap:
  Nova binds no ICU4X, so ECMA-402 is the one axis where adding a binding (or leaning on
  Boa for the wasm/fullweb tier) is a real decision. The principle is "neither engine
  hand-rolls Temporal/Intl; both wrap the canonical crates," not "Boa owns them."
- **Nova owns ECMA-262 core (already ~96%), the built-ins clusters, the regex tail,
  and realms/host-hooks.** These are the native-hot-path obligations for content
  actors, where Nova is the primary engine.

This is not "Boa owns Intl/Temporal forever." Both are required for the fullweb goal,
and both reduce to binding canonical crates. Temporal is upstream trynova's active work
(ride it). Intl is the **non-redundant** one: no one is binding ICU4X into Nova, so it is
the conformance target where fork effort uniquely advances native fullweb — behind a
fullweb feature gate, so it never weighs the lower tiers.

## The work program

Reordered from the original seven so architectural clusters precede leaves, and scoped
per the split above.

**Precondition (2026-07-09): rebase the fork onto upstream trynova first,
snapshot-clone-preserving.** Before opening any Temporal or built-ins cluster,
re-measure the rebase delta (the "no windfall" reading is a month stale, above)
and harvest upstream's accrued `temporal_rs` + built-in bindings rather than
hand-binding in-fork. The rebase is **gated on preserving the fork-local
patches**, which are genet's fork identity, and one of which is load-bearing for
another lane:

- `GcAgent::snapshot_clone` + the actual-stack-use guard
  (`AgentOptions::stack_limit_bytes`, nova `cce0f09b`): the WPT harness plan's
  H2a / Nova-scored throughput lane depends on it, so a rebase that drops it
  regresses broad `dom --engine nova`
  (`2026-06-24_wpt_harness_exactness_plan.md`, H2). This is the coupling that
  makes the rebase "snapshot-clone-preserving," not merely a version bump.
- the `regress` regex swap (2026-06-02, 244 improvements) and the WTF-8/UTF-16
  lone-surrogate indexing fixes (2026-06-02).
- the Promise-combinator iterator-close fix (nova `e9765334` + `b5201d12`,
  which closed the hang family + 16 gaps with zero regressions).
- the wasm64 `Value`-size lane (the fork identity per the vano fork posture).

**Rebase done-condition:** every fork-local patch carried forward or upstreamed,
and both guards green post-rebase: the WPT `dom --engine nova` baseline and the
test262 committed `expectations.json`. Fold work item 5's `regress` upstreaming
into this so the fork stops re-carrying the swap across future rebases.

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
   that remove classes of expected failures: `Promise` (incl. subclassing), `Iterator`,
   `Set`, `TypedArray`, `DataView`, `Atomics`, and module/import behavior. Chase clusters,
   not low-hanging leaves. (`Temporal` is the biggest cluster by far but rides upstream
   trynova — track, do not duplicate; see the Step 0 result.)
4. **Realms and host hooks.** The contributing doc calls realm-specific heap work an
   active design area. For content actors, cross-realm objects, modules, jobs/microtasks,
   and host hooks are what turn "looks like JS" into "acts like JS." genet already
   exercises this seam (the `HostHooks` job queue + the host-promise primitive), so it
   is partly load-bearing already.
5. **Close the RegExp tail and upstream the swap.** Sticky-`y` native support,
   lone-surrogate pattern source, then push `regress` to trynova so the fork stops
   carrying it.
6. **Differential fuzzing, once the dashboard climbs.** Compare Nova against
   V8/SpiderMonkey/JSC (and against Boa, the in-tree oracle) on generated programs.
   test262 catches spec obligations; fuzzing catches interactions between them.

Intl/402 is the one item Boa covers (the wasm/portable tier) that Nova does not. For
**native** fullweb it is the non-redundant Nova binding to build (ICU4X, fullweb-gated);
see Scope below.

## Scope: profile tier and sequencing

Fullweb is the destination, not an optional far-tier: genet is a selective
(static-to-fullweb), vello/wgpu, wasm-safe web rendering engine, and **total conformance
including ECMA-402 is the goal**, on the same genet / xilem-serval spine as meerkat
chrome. So 402 is required, not deferrable-forever — the question is sequencing, not
whether.

The profile ladder (static / interactive / scripted / fullweb) is the staging, and the
near-term front line is **meerkat chrome and the rendering stack** (layout / paint /
compositor / the gc-arena-DOM work), the product's differentiated core. Scripting
conformance is **parallel required infrastructure** on the same substrate:

- **Lower tiers carry no Intl** (static / interactive / scripted need none), so an ICU4X
  binding is **fullweb-feature-gated** and never weighs them. The selective architecture
  is exactly what makes adding a heavy Intl dependency cost-free for the tiers that do not
  want it.
- **Intl is the non-redundant Nova binding** for native fullweb. Boa already covers the
  wasm/portable fullweb path (it binds ICU4X); Nova is the native gap. So when the
  scripting-conformance axis gets a focused slice, Intl is the pick: required for the
  goal, unclaimed upstream, a clean ICU4X bind, and a large block (~3,300 tests).

## Relationship to existing docs

- [Script engine plan](./2026-05-20_genet_script_engine_plan.md) owns the
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
- genet fork state: the regress and WTF-8 docs above; Boa's ICU4X/Temporal stack
  observed in the `script-engine-boa` build graph.
