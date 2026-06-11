# JS execution strategy — JIT vs AOT vs partial evaluation (weval)

Status: **research note, 2026-05-25.** Not a committed decision — captures
the analysis + a proposed direction + its sidequests, pitfalls, and
contradictions, for review. Supersedes parts of
[2026-05-20_serval_script_engine_plan.md](./2026-05-20_serval_script_engine_plan.md)
if adopted (see [Contradictions](#contradictions-to-reconcile)).

## The question

How does serval run JS across its profiles — **native** (desktop app) and
**wasm** (PWA / extension) — given that a browser wasm sandbox forbids a
native JIT (no W^X, no runtime native codegen)?

## The mechanisms, honestly ranked

| Mechanism | Sandbox-legal? | Cost | Verdict |
| --- | --- | --- | --- |
| **JIT → native** (Cranelift/Winch as a JS JIT) | ❌ in browser (W^X); native only | Megaproject: inline caches, deopt, type feedback, GC-integrated codegen | Out for wasm; off the Nova interpreter bet for native |
| **AOT JS → wasm** (Porffor) / **AOT typed-JS → native** (Static Hermes) | ✅ (compilation is data-transformation; output runs on the host engine) | A **hand-written JS→wasm compiler**, Porffor-scale; untyped JS AOTs poorly (best with types) | Applicable, but a large from-scratch build |
| **Partial evaluation (weval)** — Futamura projection | ✅ (same: produces wasm) | Reuse an **existing bytecode interpreter** + annotate its dispatch loop; the partial evaluator generates the compiler-equivalent | **The pick** — no compiler to write, no JIT, no type requirement |

**Why weval wins the framing:** the [first Futamura projection](https://en.wikipedia.org/wiki/Partial_evaluation)
says *specializing an interpreter against a specific program yields a
compiled program*. So instead of writing a JS→wasm compiler, you take a
bytecode interpreter you already have (Boa, or Nova), mark its dispatch
loop's bytecode pointer as the specialization context, and weval emits a
specialized wasm module with the interpreter's dispatch overhead compiled
away. The browser's own wasm engine then optimizes that wasm — you borrow
the host's JIT instead of shipping one. Crucially, weval gets its win by
eliminating *dispatch* (not by type-specializing values), so **it doesn't
need a typed dialect** to help — unlike hand-AOT.

## Codebase facts (verified) vs external claims (verify upstream)

Being explicit about what's grounded vs. recalled, per doc policy.

**Verified against the tree:**

- Nova's `Value` is word-sized — `const _VALUE_SIZE_IS_WORD: () =
  assert!(size_of::<Value>() == size_of::<usize>());`
  (`crates/nova/nova_vm/src/ecmascript/types/language/value.rs:398`). So
  wasm32 (usize=4, Value=8) **fails to compile**; native/wasm64 (usize=8)
  **passes**. This is *the* "64-bit wall," and it's exact, not vague.
- `usdt` is a **hard** dep of `nova_vm` (`Cargo.toml:39` + build-dep `:109`)
  — won't build on wasm; needs a fork-gate.
- `ecmascript_atomics` is **optional** (behind `array-buffer` /
  `shared-array-buffer`) — gate off for wasm.
- Boa is blocked from serval's workspace by `icu_normalizer ~2.0` vs
  parley's `^2.1.1` — a fork-and-bump resolves it.

**External claims (training knowledge, ~Jan 2026 — verify against current
upstream before relying):**

- **weval** = Chris Fallin's (Cranelift author) WebAssembly partial
  evaluator; the Bytecode Alliance uses it over SpiderMonkey in
  StarlingMonkey/ComponentizeJS to get fast JS-in-wasm with no JIT;
  reported multi-x speedups over plain interpretation. Battle-tested
  consumer is **C++** (SpiderMonkey); Rust support exists but is less
  proven.
- **Porffor** = AOT JS→wasm compiler, pre-alpha (full-JS coverage is the
  hard part). **Static Hermes** = AOT typed-JS→native (Meta), the maturity
  proof that AOT-JS works *with types*.
- **memory64** shipped in Chrome + Firefox; the Rust `wasm64-unknown-unknown`
  target is **tier-3** (nightly + `-Z build-std`, partial std). memory64
  also carries a **bounds-check perf tax** (no cheap 32-bit guard-page trick).

## Proposed direction

- **Native (desktop):** **Nova** — full engine, 64-bit `Value`, all native
  deps on (usdt, atomics, real getrandom). The unconstrained reference
  interpreter; ride upstream Nova's interpreter-level perf work, no JIT.
- **wasm (PWA / extension):** **Boa, forked** (icu pin bump **+** weval
  dispatch intrinsics) → wasm32 → **weval-specialized** at build time
  against shipped JS → fast wasm the browser optimizes. Plain Boa stays as
  the `eval`/dynamic-code fallback. Boa keeps its conformance-oracle role
  on top (now double duty).
- **No JS JIT anywhere.** Cranelift re-enters *only* if serval runs wasm
  **components/plugins** natively (Wasmtime backend, or `.cwasm` AOT) — a
  separate subsystem from either JS engine.
- **Nova-on-wasm64 / "one engine everywhere":** deferred. weval removes the
  *urgency* (Boa-on-wasm32 gets the speed without the tier-3 toolchain risk
  + memory64 tax). Revisit if the wasm64 Rust toolchain matures **and** Nova
  sheds its native-only deps.

## Why our own JS engine in-browser at all (the load-bearing question)

A PWA already has a world-class JS engine — the host browser's. Why ship
Boa-as-wasm instead of using it? Because **serval is the web platform**:
content JS must run against **serval's** reimplemented DOM (its box-tree /
layout / event model), not the host page's DOM. The host's JS engine binds
to the host's object model; using it would mean reimplementing the entire
JS↔DOM binding layer over a foreign engine's assumptions. A self-contained
engine (Boa/Nova) bound to serval's DOM is the clean approach. *(Rejected
alternative: bridge content-JS to the host engine via serval-DOM host
functions — fights the host's object model; brittle.)* The cost of this is
real — see the perf-ceiling pitfall.

## Sidequests

- **wizer pre-initialization** — weval's sibling (also Fallin/BA): snapshot
  the wasm module post-init for faster startup. Orthogonal easy win for any
  serval-in-wasm build, weval or not.
- **Typed dialect + weval stacking** — weval kills dispatch overhead; a
  typed scripting dialect would *also* kill type-dispatch, stacking toward
  JIT-grade perf. Connects to the "we control our scripting profile"
  lever — a green-field platform can mandate types where the legacy web
  can't.
- **weval → wasm → Wasmtime `.cwasm`** — a single weval'd-wasm artifact run
  natively (Wasmtime AOT) *and* in-browser, unifying both profiles' JS on
  one pipeline. Tempting, but probably loses to Nova-native on the desktop;
  note and likely skip.
- **Boa-bytecode → wasm direct lowering** — the alternative to weval (write
  the AOT backend over Boa's bytecode IR). More control, much more work +
  maintenance than letting the partial evaluator do it.
- **Rhai on wasm** — Rhai (Mere's *app* scripting) is AST-walking, not
  bytecode, so it's a **poor weval fit**; note the mismatch (don't assume
  weval helps every interpreter).
- **Winch vs Cranelift** — only if native wasm-component execution lands:
  Winch (fast baseline) for startup, Cranelift (optimizing) or `.cwasm`
  AOT for throughput.

## Pitfalls

- **weval Rust integration is unblazed.** The proven consumer is
  SpiderMonkey (C++). Wiring weval into Boa's Rust bytecode VM is real
  integration risk, not just "add intrinsics."
- **weval needs a specific interpreter shape.** Context-threaded dispatch.
  Boa's VM may need *restructuring*, not just annotation — the fork could be
  bigger than a pin-bump-plus-markers.
- **Build-time specialization ≠ dynamic JS.** weval specializes against
  *known* programs. `eval` / `new Function` / dynamically-fetched scripts
  fall back to the plain interpreter (slow). Coverage is best for
  shipped/static content (smolweb, bundled extension code); thinner for
  fullweb's dynamically-loaded JS.
- **Perf ceiling.** weval'd-JS-as-wasm runs at the browser's *wasm* speed —
  above an interpreter, but **below the browser's native JS** (V8/SpiderMonkey
  JS JITs are extraordinarily tuned). This is the inherent tax of running a
  reimplemented platform's JS in wasm; weval narrows the gap, doesn't close
  it.
- **Code size.** Per-program specialization can bloat the wasm — a PWA
  download-size concern.
- **Two-engine conformance drift.** Boa-wasm vs Nova-native must stay
  behaviorally aligned. The oracle infra mitigates but doesn't erase the
  maintenance/correctness tax of two JS engines.
- **Deep fork maintenance.** Instrumenting Boa's *dispatch loop* is a far
  deeper fork than the icu pin bump — harder to track against upstream Boa,
  tension with the "don't churn deps / keep forks thin" stance.
- **Bus factor.** weval is largely one person + BA. A load-bearing JS path
  on it is a supply-chain risk (mitigate: offer-don't-push contributions;
  keep the plain-interpreter fallback genuinely viable).
- **Debuggability.** Source-mapping through partial evaluation to the
  original JS is hard — a DevX cost for the weval'd fast path.

## Contradictions to reconcile

1. **"The wasm target is the no-JS profile"**
   ([script-engine plan L47](./2026-05-20_serval_script_engine_plan.md), L657:
   *"the wasm-safe profile = the no-JS profile"*) → **overturned.** wasm
   becomes the **Boa+weval JS** profile.
2. **"Boa stays only as a conformance oracle"** (script-engine plan L50) →
   **Boa is promoted** to the wasm-profile engine (double duty: oracle +
   wasm JS). The "quarantine" framing goes.
3. **"Nova is the primary backend"** (L566) → **qualify per profile**: Nova
   = native engine; Boa = wasm engine. "Primary" is native-primary.
4. **"Rhai works everywhere wasm32 ships"** (memory:
   `project_browser_pwa_shapes_scripting`) → **disambiguate, not a real
   conflict:** Rhai is **Mere's app/extension scripting**; Boa/Nova is
   **serval's web-content JS**. Two distinct scripting domains that can both
   live on wasm — but the docs read like a contradiction until that split is
   stated explicitly.
5. **"Wait-and-see: memory64 vs the Boa pin bump"** (earlier stance) →
   weval **reduces the wasm64 urgency** (Boa-on-wasm32+weval gets speed
   without wasm64). Recalculate: the pin bump + weval is the nearer, safer
   path than chasing tier-3 wasm64.
6. **"No Cranelift in the JS path"** (this thread, earlier) → **still true**:
   weval ≠ Cranelift, and Cranelift re-enters only for native wasm-component
   execution, not JS. The earlier "no" was about *JIT-to-native*; AOT/weval
   is a different axis.

## Open questions / next steps

- **Verify weval upstream** — current Rust support, the dispatch-loop shape
  it requires, maturity, and whether a Rust bytecode VM (Boa) has been weval'd
  before.
- **Prototype** — instrument Boa's bytecode loop with weval intrinsics on a
  small JS program; measure speedup vs plain Boa-in-wasm. (Mirrors the
  `nova-probe` pattern: a standalone probe before committing the fork. The probe
  crates themselves were removed 2026-06-10 once the real `components/script-engine-*`
  crates subsumed their proofs as live tests; the pattern stands.)
- **Name the scripting-domain split** — Rhai (app) vs Boa/Nova (content JS)
  — explicitly, so the "Rhai on wasm" memory and this note stop reading as
  contradictory.
- **Decide native wasm-component story** (separate) — does serval/Mere run
  wasm plugins natively? If yes, that's the only place Cranelift/Wasmtime
  AOT belongs.
