# serval script engine plan — modular JS backends, prerender lane, WPT trajectory

**Status (2026-05-20):** proposed; for review. Designs the scripting layer that the
[profile-ladder plan](./2026-05-12_serval_profile_ladder_plan.md) left as the
`serval-scripted` tier's open interior, after the 2026-05-15 SpiderMonkey
excision (see [workspace audit snapshot](./2026-05-16_workspace_audit_snapshot.md)).

**Now (2026-05-24): partially IMPLEMENTED — not "no implementation yet".** The design
below is built through the incremental-layout core: `script-engine-api` + the Nova
(primary) and Boa (oracle) backends, `serval-scripted-dom` + the reflector bridge, and
both coarse and incremental relayout — all committed and diff-tested (see the dated
build records inline below, and the [2026-05-24 state snapshot](./2026-05-24_workspace_audit_snapshot.md)).
Remaining: the fine-grained Stylo-restyle arc and the incremental edges.

**Memory64 authority (2026-06-24):**
[`2026-06-24_nova_memory64_browser_lane_plan.md`](./2026-06-24_nova_memory64_browser_lane_plan.md)
supersedes every "Nova native-only", "no JS in wasm", and "browsers do not run
Memory64" conclusion below. Nova is 64-bit-target-only; the Nova/wasm64 and
Boa/wasm32 worker artifacts now both build behind binary capability detection.
The older passages remain only as decision history.

**Revised 2026-05-20 (review pass).** Six corrections, all incorporated below:
(1) `mark_dirty` removed from `LayoutDomMut` — DOM mutations emit `DomMutation`
records; serval-layout (not the DOM provider) translates them into
StylePlane/LayoutPlane invalidation. (2) `script-engine-api` trimmed to a pure VM
trait; browser host surface (document/timers/fetch/bootstrap/fake-DOM/event-loop)
moved up into a new `script-runtime-api` layer. (3) the VM trait is an
object-*unsafe* generic compile-time trait; runtime A/B needs an explicit
`Backend` dispatch enum, now stated rather than implied. (4) prerender gets an
explicit **sandbox/capability** rule — hostile remote JS runs in a constrained
worker/process/wasm sandbox; a VM interrupt is not a trust boundary. (5) prerender
reframed as a Serval **tier-selection escalation**, not a Hekate E3 extraction
mode. (6) the WPT/test262 framing de-mathematized — test262 is a diagnostic
baseline for the engine axis, not a literal WPT cap.

**Probe appended 2026-05-20.** [Appendix A](#appendix-a--engine-api-paper-probe-2026-05-20)
is a paper probe of the VM trait against current **rquickjs 0.11** and **boa 0.21**
(not rakers' 0.8/0.19). Verdict: trait holds, with two consequences folded back in
— `new_function` mandates explicit per-backend-bounded captures, and live-DOM
GC needs a `ScriptEngineLive` reflector extension. Resolves open question #1.

**Direction set 2026-05-21.** **Nova is the primary backend**; Boa is retained as a
**conformance oracle** (run the same bindings against Boa's higher test262 to bucket
failures as Nova-spec-gap vs binding-bug). Rationale, and the arena/target re-axing
it unlocks, are in [Part 6](#part-6--nova-primary-and-arena-composition).
Nova facts corrected: it shipped **1.0** (2026-03-15), passes **~80% test262**, is
**NLnet-funded** — *not* the "immature/early" the earlier draft called it. Its
README's "no WebAssembly support" means it can't *execute* `WebAssembly.*` from JS (a
gated fullweb feature) — a separate axis from whether `nova_vm` compiles *to* wasm32.
**On that axis the verdict flipped (2026-05-22, [Appendix B](#appendix-b--wasm32-verification-2026-05-21)):
`nova_vm` does NOT support wasm32 — its data-oriented `Value` is `usize`-sized and its
arithmetic assumes 64-bit, which breaks on 32-bit wasm. Not a gate; core upstream work.**
That 2026-05-23 conclusion is now superseded: Nova is **64-bit-target-only**, and the
experimental wasm64 worker lane ships beside the Boa/wasm32 compatibility worker. The wasm target is no longer restricted to the **no-JS profile**
(structured HTML + smolweb — the browser + middlenet offering, robust on its own); if
you're in a browser you already have a JS engine, and where Mere genuinely needs JS it is
the *native* host (Nova). Boa stays only as a **native conformance oracle, if/when** its
icu pin clears (currently blocked — Part 3). We **wait** on memory64-or-the-pin rather than
fork Boa or pivot to quickjs. (This supersedes the earlier "Boa is the wasm backend"
framing in the matrix/Appendix B below — there is no wasm JS backend.) Caveat kept honest:
1.0 does **not** mean a frozen API
— upstream plans frequent breaking major bumps and no LTS, so the binding layer
signs up to track churn (the Xilem deal, knowingly taken).

The cut left the scripting slot empty *and clean*: `components/script`,
`script_bindings`, `dom_struct`, `jstraceable_derive` are gone; `mozjs` survives
only as an orphaned pin at [Cargo.toml:136](../Cargo.toml) with a commented-out
path-dep stanza below it. This plan is about what fills that slot, and how it
stays a *free variable* rather than re-marrying Serval to one engine.

Sibling reads:

- [2026-05-12 profile-ladder plan](./2026-05-12_serval_profile_ladder_plan.md) — the static/interactive/scripted/fullweb tier framing; this plan is the scripted interior.
- [2026-05-16 layout_dom_api design](./2026-05-16_layout_dom_api_design.md) — the DOM-side contract; its **open question #1 (mutation surface)** is resolved here.
- [2026-05-17 hekate lanes + observables](./2026-05-17_hekate_lanes_observables.md) — the cross-engine "common-minimum trait + extensions" and "witness by package" doctrines this plan reuses.

Prior art read for this plan: **[tbro/rakers](https://github.com/tbro/rakers)**
(MIT, v0.1.5, May 2026) — a Rust headless JS renderer whose engine is
compile-time pluggable (`rquickjs` default, `boa` alt) over an html5ever DOM.
Its `runtime.rs` / `bootstrap.js` are the direct template for the **prerender
mode** below.

---

## Thesis — the engine is the cheap part

The thing that made `components/script` 772 files was never SpiderMonkey itself;
it was the **binding bridge**: WebIDL codegen → JSAPI rooting (`MutDom<JSVal>`),
`JSTraceable`, `JSContext` threaded through every DOM op, finalizers linking the
JS heap to DOM-node lifetimes. Swapping engines touches ~10% of that cost.
Rebuilding the bridge is the other 90%.

The 2026-05-15 cut already paid down the hard prerequisite. `serval-static-dom`
behind the opaque `layout-dom-api` `NodeId` trait means the DOM is no longer
*married* to JSAPI rooting. That seam is exactly where a scripting layer plugs
in. Pre-cut, you couldn't have chosen anything but SpiderMonkey because the DOM
**was** the bindings. Post-cut, the engine is genuinely swappable. This plan
keeps it that way.

### Two different things both called "script"

| | `ScriptThreadFactory` (existing) | `ScriptEngine` (this plan) |
| --- | --- | --- |
| Where | `components/shared/script` (`script_traits`), [lib.rs:413](../components/shared/script/lib.rs) | new `script-engine-api` crate |
| Shape | constellation/IPC/pipeline-shaped: `InitialScriptState`, `ScriptThreadMessage`, `LayoutFactory`, BHM register | engine-backend-shaped: eval, host-object registration, microtask pump |
| Weight | fullweb orchestration | one JS VM + its DOM binding |
| Heap coupling | Servo's whole script-thread model | none beyond the chosen VM |

`ScriptThreadFactory` is the **fullweb** path's seam to the constellation. It is
not the engine abstraction. The relationship is: a future `serval-scripted`
`ScriptThreadFactory` impl is built *on top of* a `ScriptEngine` backend — the
factory owns pipelines/lifecycle, the engine owns "run this JS against this DOM."
This plan defines the lower layer and explicitly defers the constellation
re-integration to the fullweb tier (profile-ladder P4/P6).

---

## Execution modes map onto the profile ladder

The ladder already names the tiers. Scripting adds a mode the ladder didn't
distinguish: **prerender** sits *below* live scripting and needs **no binding
bridge at all**.

| Mode | Profile tier | DOM model | Memory managers | Engine bindings | Engine req'd |
| --- | --- | --- | --- | --- | --- |
| **static** | `serval-static-html` / `-interactive-html` | live Rust DOM, no JS | one (Rust) | none | none (mozjs-free, audit canary) |
| **prerender / snapshot** | new sub-mode (see below) | *throwaway JS-side DOM*, reserialize → `StaticDocument` | one (the JS VM) | **none** | any wasm-capable VM |
| **live scripted** | `serval-scripted` | real Serval DOM mutated through bindings; layout reacts | two (the marriage) | full `LayoutDomMut` bridge | any |
| **fullweb** | `serval-fullweb` | live + workers/storage/media/etc. | two + service heaps | full + platform APIs | near-100%-conformant VM |

The key insight from rakers: **prerender has one heap, not two.** The DOM the
scripts touch *is* JS objects, GC'd by the VM alone; the real DOM and the
script-time DOM never coexist as a live mutable graph. That sidesteps the entire
GC-marriage problem (rooting, `JSTraceable`, cross-heap cycles) for a large class
of real sites (SSR/SPA initial-render). It is leak-free by construction and needs
zero `layout-dom` mutation. It is the cheap on-ramp the ladder was missing.

Prerender is **not** a security/fidelity equal of live scripting — it runs the
page's JS *once* against a faked DOM and captures the HTML produced. No
interaction, no post-load mutation, no event loop beyond initial settle. It buys
"pages that need JS to emit their first paint," not "interactive web apps."

---

## Part 1 — the `ScriptEngine` backend trait

### Crate layout (witness by package, per ladder doctrine)

Cargo features unify; a low profile hidden behind `--no-default-features` is not
safe if another crate re-enables a default. The ladder's answer is **package
witnesses**. Apply the same to engines:

```text
# Layer 0 — the VM. Knows nothing about browsers.
script-engine-api          # minimal VM trait + `Backend` dispatch enum. No VM dep, no browser concepts.
script-engine-nova         # impl over nova_vm 1.0. PRIMARY backend, NATIVE-ONLY. pure Rust, handle-arena; not wasm32 (64-bit-bound, Appendix B).
script-engine-boa          # impl over boa_engine 0.21. CONFORMANCE ORACLE (~94% test262). pure Rust, wasm-ok.
script-engine-quickjs      # impl over rquickjs (quickjs-ng). optional alt; wasm-ok, small C dep.
script-engine-spidermonkey # impl over mozjs. NATIVE ONLY, brings the build env back. fullweb-only, if ever.

# Layer 1 — the browser host surface, built ON the VM primitives.
script-runtime-api         # document/console/timers/fetch stubs, bootstrap JS, event-loop driver,
                           # the fake/real DOM host objects. This is where "browser-ish" lives —
                           # NOT in script-engine-api, or the VM trait quietly becomes a host API.
```

The layering rule is load-bearing: `script-engine-api` exposes only VM
capabilities (eval, value conversion, native-callback creation, global
get/set, microtask pump, deadline). Everything browser-shaped — `document`,
timers, `fetch` shims, the event loop, `bootstrap.js`, the fake-DOM (prerender) or
real-DOM (live) host objects — is assembled *one layer up* in `script-runtime-api`
out of those primitives. If `document` or a timer ever appears in
`script-engine-api`, the boundary has failed.

A wasm32 build of `serval-scripted` depends on the quickjs/boa/nova impls and
**structurally cannot** name `script-engine-spidermonkey` — not because a feature
is off, but because the crate isn't in the graph. That's the ladder's
"witness, don't gate" rule applied to the engine axis. The audit canary extends:
`serval-static-html` / `-interactive-html` pull *no* `script-engine-*` crate at
all; the wasm `serval-scripted` graph contains no `mozjs`.

### Trait shape (illustrative-signature-only — not compile-ready)

The trait is deliberately small — only VM capabilities. No `document`, no timers,
no DOM. The browser host surface is built in `script-runtime-api` out of
`new_function` + `set_global`.

```rust
// script-engine-api/lib.rs  — ILLUSTRATIVE. Real bounds (Send/Sync, lifetimes,
// the native-callback type) are decided at scaffold time.

/// One JS VM instance. Backends: quickjs, boa, nova, spidermonkey.
/// Object-UNSAFE by construction (associated `Value`/`Error`, `Self`-typed
/// callbacks) — it monomorphizes per backend. See `Backend` below for runtime
/// selection; there is no `dyn ScriptEngine`.
pub trait ScriptEngine: Sized {
    type Value;                       // engine-native value handle
    type Error;

    /// Construct a VM. `limits` carries stack/recursion/opcode-budget knobs;
    /// backends that can't honor one (e.g. boa has no interrupt handler today)
    /// accept-and-ignore, documented per impl.
    fn new(limits: EngineLimits) -> Result<Self, Self::Error>;

    /// Evaluate source in script or module mode (sloppy by default — frameworks
    /// assign undeclared globals; rakers learned this the hard way).
    fn eval(&mut self, source: &str, mode: EvalMode) -> Result<Self::Value, Self::Error>;

    // --- value conversion (Rust <-> engine) ---
    fn new_string(&mut self, s: &str) -> Result<Self::Value, Self::Error>;
    fn to_rust_string(&mut self, v: &Self::Value) -> Result<String, Self::Error>;
    // ... numbers / bools / objects / arrays as the binding layer needs them

    /// Create a native callback as an engine value. THE primitive the runtime
    /// layer composes every host object from; the VM knows nothing of `document`.
    /// Captures are EXPLICIT and per-backend-bounded (`EngineCaptures`: rquickjs =
    /// any `'static`; boa = `Trace + 'static`) — implicit closure capture is
    /// forbidden because boa's GC can't trace it. See Appendix A, Finding 1.
    fn new_function<C: EngineCaptures<Self>>(&mut self, captures: C, f: NativeFn<Self, C>)
        -> Result<Self::Value, Self::Error>;

    /// Get/set a property on the global object, so the runtime layer can install
    /// whatever globals it wants. A VM primitive, not a browser concept.
    fn set_global(&mut self, name: &str, value: Self::Value) -> Result<(), Self::Error>;

    /// Run one batch of pending microtasks (Promise jobs). Returns whether any ran,
    /// so the runtime layer can interleave timer flushes (the rakers Backburner lesson).
    fn pump_microtasks(&mut self) -> bool;

    /// Cooperative deadline for runaway-script protection. NOT a security boundary
    /// (see the prerender sandbox section). Backends without preemption (boa)
    /// return `Unsupported`.
    fn set_deadline(&mut self, deadline: Option<Instant>) -> DeadlineSupport;
}

/// Runtime backend selection — the dispatch shim for the object-unsafe trait.
/// Each arm is a distinct monomorphization; this is how the WPT runner A/Bs
/// backends in one process. It is NOT `Box<dyn ScriptEngine>`.
pub enum Backend {
    #[cfg(feature = "quickjs")]      QuickJs(QuickJsEngine),
    #[cfg(feature = "boa")]          Boa(BoaEngine),
    #[cfg(feature = "nova")]         Nova(NovaEngine),
    #[cfg(all(feature = "spidermonkey", not(target_family = "wasm")))]
                                     SpiderMonkey(SpiderMonkeyEngine),
}
```

**Where the GC marriage lives.** For prerender, native callbacks are state-free
shims and the DOM they touch is pure JS — one heap, no marriage; `new_function`
with explicit non-JS captures is the whole surface. For live scripting it's more:
the Appendix A probe shows the DOM↔JS cycle is only collectable if each DOM node's
JS **reflector** is a traced native object (rquickjs `Class<T>` / boa `JsClass`),
behind a `ScriptEngineLive` extension trait — not plain `new_function`. Either way
the engine types live in the `script-engine-*` crates and never surface in
`script-engine-api`'s public values.

### What rakers proves about the trait

rakers' `JsRuntime` (concrete type, `compile_error!` if both engines on) validates
the surface against *two very different memory models* (boa tracing GC vs quickjs
refcount+CC). Three deltas for Serval:

1. **Trait + dispatch enum, not concrete + mutually-exclusive features.** We want
   to *support* four backends, compare them, and gate by target. The trait is
   object-unsafe and monomorphizes per backend; the `Backend` enum above is the
   runtime dispatch shim that lets the WPT runner A/B them in one process. Features
   gate *availability* per crate, not exclusivity. (rakers' `compile_error!`-if-both
   is the right call for a single-engine CLI; it's the wrong call for a
   multi-backend conformance harness.)
2. **Kill the `thread_local!` output capture.** rakers stashes
   `WRITTEN`/`LOGGED`/`BODY_INNER_HTML` in thread-locals because native callbacks
   can't easily close over Rust state across FFI. Both engines have a clean
   answer (rquickjs `Ctx` userdata, boa `HostDefined`). Live scripting *needs*
   this — callbacks must reach the real DOM, not a thread-local string.
3. **`bootstrap.js` is the host-object inventory, for free.** 758 lines of "what
   globals real frameworks touch" (`process.env.NODE_ENV`, `node.ownerDocument`
   for React delegation, `classList`/`attributes` for Elm/Angular/Vue, sloppy
   mode for SvelteKit/webpack). It's the prerender host surface verbatim and the
   *checklist* for the live-binding surface.

---

## Part 2 — prerender mode (lift rakers, adapted)

New crate `serval-prerender`, depending on `script-engine-api` (+ one backend) and
`serval-static-dom`. **Not** on `layout-dom`'s mutation surface — it doesn't have
one.

Pipeline (rakers-shaped):

```text
HTML bytes
  → html5ever parse → extract <script> sources in document order  (serval-static-dom already parses; reuse its sink)
  → ScriptEngine: eval bootstrap (JS-side faux DOM) → eval scripts → pump microtasks + flush timers to settle
  → read back document.body.innerHTML  (a JS string)
  → re-parse that string into StaticDocument
  → hand to serval-layout planes → Paint  (the normal static path)
```

What to lift directly: the bootstrap globals, the microtask/timer drain loop
(the `while pump_microtasks() {}` interleaved with timer batches — the Ember/
Backburner lesson that both queues must drain together), the quickjs
opcode-budget interrupt handler.

What to change: trait-backed engine selection; context-struct state instead of
thread-locals; the readback feeds `serval-static-dom` rather than a CLI string.

### Sandbox & capability boundary (mandatory for remote content)

Prerender runs **arbitrary, often hostile, remote JS** — and Graphshell/Mere/Serval
run it next to local-first state. A `set_deadline` opcode-budget interrupt bounds
*runtime*; it is **not a trust boundary**. The rule, established now:

- **Remote prerender runs in a constrained sandbox** — a separate worker/process,
  or a wasm sandbox — with **explicit, allow-listed host capabilities** (the
  prerender host surface grants timers, `document` shims, and an *optional,
  policy-gated* `fetch`; it grants nothing else — no filesystem, no local IPC, no
  ambient network beyond the page's own origin policy).
- **`fetch`/XHR in prerender is opt-in and brokered.** rakers' `_r_fetch_sync`
  does live synchronous GETs by default; for Serval that capability is off unless
  the route's policy grants it, and when granted it goes through the host network
  broker (subject to the same origin/TLS rules as a real load), never a raw socket
  from inside the VM.
- **Local fixture prerender may run in-process** (test corpus, trusted content) —
  the in-process path is a *fixture/dev* affordance, not the remote path. The
  capability set is the discriminator, not the transport.
- **The boa backend can't run remote content in-process at all** (Appendix A,
  Finding 3): boa 0.21 has no interrupt/fuel, so a runaway script cannot be bounded
  in-VM — only an externally-killable worker/process contains it. rquickjs has a
  cooperative interrupt as the first line, with process-kill as backstop.
- The sandbox is a `script-runtime-api`/host concern, **not** a `script-engine-api`
  one. The VM trait stays unaware; the runtime layer decides where the VM runs and
  what it can reach.

This applies to live scripting too (more so), but prerender is where it's easy to
under-build because "we just render once" feels harmless. It isn't.

### Hekate tie-in (tier selection, not extraction)

Prerender is a **Serval tier-selection escalation**, not a Hekate extraction tier.
Keep these distinct (the hekate doc's vocabulary depends on it): E0–E4 are
*extraction* tiers Hekate owns (E3 = style-assisted extraction, E4 = layout-assisted);
the static/prerender/scripted/fullweb tiers are *render* selections Serval owns.

The connection is a **route-hint, not an E3 mode**: Hekate's signals ("app-shell
HTML, empty `<div id=root>`, SPA skeleton with no real content in the initial
HTML") are route/tier-selection *evidence*. Hekate passes that as a tier hint when
handing the document to the Serval lane; **Serval** decides to instantiate the
prerender tier, and can re-escalate to live scripting if interaction is later
required (reporting the re-escalation back to Hekate as route-hint evidence, per
the hekate doc's mid-session escalation rule).

---

## Part 3 — live scripting: resolving the mutation seam

This resolves **open question #1** in the [layout_dom_api design](./2026-05-16_layout_dom_api_design.md):
mutation goes in a `LayoutDomMut: LayoutDom` **extension trait**, not in the base
trait and not a separate parallel trait.

```rust
// components/shared/layout-dom/lib.rs — ILLUSTRATIVE addition.
pub trait LayoutDomMut: LayoutDom {
    fn create_element(&mut self, name: QualName) -> Self::NodeId;
    fn create_text(&mut self, data: &str) -> Self::NodeId;
    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId);
    fn remove(&mut self, node: Self::NodeId);
    fn set_attribute(&mut self, node: Self::NodeId, name: QualName, value: &str);
    fn set_text(&mut self, node: Self::NodeId, data: &str);
    /// innerHTML setter: parse fragment, replace subtree. The hot scripted path.
    fn set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), ParseError>;

    /// Drain the structural mutations recorded since the last call. The provider
    /// records WHAT changed; it has no notion of dirty bits, style, or layout.
    /// (A registered sink is an equivalent shape; pick at scaffold time.)
    fn drain_mutations(&mut self, out: &mut Vec<DomMutation<Self::NodeId>>);
}

/// What the DOM provider records — render-state-free. No `DirtyKind`, no style.
pub enum DomMutation<Id> {
    Inserted { node: Id, parent: Id },
    Removed { node: Id, former_parent: Id },
    AttributeChanged { node: Id, name: QualName },
    CharacterDataChanged { node: Id },
    SubtreeReplaced { node: Id },   // innerHTML
}
```

Why an extension trait:

- `serval-static-dom` / reader-mode / serialization / selector-matching consume
  `LayoutDom` and must stay mutation-free and `mozjs`-free. Putting mutation on
  the base trait would force every read-only consumer to acknowledge it.
- The scripted-DOM provider (new `serval-scripted-dom`, parallel to
  `serval-static-dom`) implements **both** `LayoutDom` and `LayoutDomMut`.
- This is where the GC marriage is real and per-backend: the `serval-scripted-dom`
  node store must cooperate with the chosen engine's collector so that
  DOM↔JS cycles (a node holding an event handler that captures the node) don't
  leak. Per the earlier analysis, quickjs's refcount+cycle-collector is the model
  a Rust node store can most naturally *join*; boa's tracing GC needs
  `Trace`/`Finalize` integration; SpiderMonkey needs JSAPI rooting. Per the Appendix
  A probe this is implemented via a per-backend **reflector** (rquickjs `Class<T>` /
  boa `JsClass`) behind a `ScriptEngineLive` extension trait — the node's JS
  reflector is a traced native object, re-deriving Servo's reflector pattern
  per-backend; plain `new_function` is not enough for the live cycle.

**Invalidation is serval-layout's job, not the DOM's.** The DOM provider emits
`DomMutation` records and nothing more — no dirty bits, no style, no layout
coupling. Putting `mark_dirty` on the DOM provider (an earlier draft did) leaks
mutable rendering state into the DOM, against the planes design where that state
belongs to serval-layout. Instead, serval-layout's scheduler **consumes** the
mutation stream and translates it into StylePlane/LayoutPlane invalidation. Stylo's
incremental-restyle machinery (`set_dirty_descendants`, snapshot bits, etc.) lives
on the serval-layout side — on the `StyleElement` adapter and its `NodeId`-keyed
side-tables, exactly where the layout_dom_api probe placed it — driven *by* the
consumed mutation stream, not by a method on the DOM trait. This is the
layout_dom_api doc's deferred "P4 problem," now owned by serval-layout's scheduler.

---

## Part 4 — WPT coverage as a target architecture

Framing the question as design + targets, not time. The core move: **separate two
conformance axes** that the monolithic "WPT pass rate" number conflates.

### The two axes

1. **Engine conformance** — does the VM implement ECMAScript correctly?
   Measured by **test262**, *fixed per backend*, not Serval's work:
   - SpiderMonkey: ~the spec.
   - quickjs-ng: very high.
   - Boa: ~94% test262 and climbing — our **conformance oracle**.
   - Nova: ~80% test262 (1.0, climbing) — our **primary**; the gap to Boa is the
     oracle's whole point (Part 6).
2. **Binding conformance** — does Serval expose the DOM/HTML/CSSOM platform
   correctly? Measured by **WPT** (`dom/`, `html/`, `cssom/`, `css/...`),
   *entirely Serval's work*, engine-independent above the language layer.

A test fails for one of these reasons, or both. Conflating them hides where the
work is. The modular-engine approach makes the engine axis **observable**: run the
WPT suite against each backend and the delta between (say) SpiderMonkey and Boa
pass rates is a *signal* of the engine-attributable gap.

Resist the clean formula, though. It's tempting to write "achievable WPT =
min(engine conformance, binding completeness)," but that's too tidy:

- **test262 is a diagnostic baseline for the engine axis, not a literal WPT cap.**
  A high language-conformance score does not bound WPT pass rate from above. Much
  of WPT lives in territory test262 barely touches — **DOM event-loop semantics,
  ES modules + dynamic import, timer ordering, URL/fetch/streams, microtask/task
  interleaving, and testharness integration itself**. A backend can ace test262 and
  still fail swaths of `dom/`/`html/` because those exercise host and event-loop
  behavior, not language features.
- So the two axes aren't independent multiplicands. The engine axis sets a *floor
  of plausibility* (a backend far below on test262 will visibly drag DOM suites);
  the binding + event-loop + platform axis is where most of the real WPT work lives
  regardless of backend.

The actionable version: **triage each WPT failure into engine-language vs
host/event-loop vs binding-breadth buckets**, and use cross-backend deltas to
confirm the engine-language attribution. Choosing Boa over SpiderMonkey for
`serval-scripted` will cost *some* DOM-test passes to language gaps — but expect
the event-loop/timer/module/harness bucket to dominate the count either way.
That's the real fullweb tradeoff, stated as triage rather than a formula.

### WPT directories map onto profiles

The WPT harness (`testharness.js`) itself needs a JS engine **and** enough DOM to
run — so any WPT run is gated on at least the prerender engine + a DOM surface.
That means:

| WPT area (examples) | Lowest profile that can attempt it | Gated on |
| --- | --- | --- |
| `css/CSS2/`, `css/css-flexbox/` reftests | `serval-static-html` (+a reftest runner that doesn't need testharness.js) | Stylo+Taffy (see [stylo_taffy plan](./2026-05-20_stylo_taffy_adoption_plan.md)) |
| `dom/nodes/`, `dom/events/` (testharness) | `serval-scripted` (live) | `ScriptEngine` + `LayoutDomMut` bindings |
| `html/semantics/` DOM APIs | `serval-scripted` (live) | bindings breadth |
| `html/dom/` reflection, `cssom/` | `serval-scripted` (live) | bindings breadth |
| `service-workers/`, `workers/`, `fetch/`, `streams/`, `storage/` | `serval-fullweb` | platform APIs + constellation |
| `webgpu/`, `webgl/` | `serval-fullweb` | canvas providers |

Two runner shapes follow:

- **Reftest runner** — renders two pages, compares pixels. Needs no JS for pure
  CSS reftests. Lives at the static tier; the first WPT signal Serval can produce
  *today* once Stylo+Taffy land. No engine dependency.
- **testharness runner** — boots a profile, loads `testharness.js` + the test,
  collects the `Tests` results object. Needs the scripted tier (live DOM). This
  is the bulk of `dom/`/`html/` coverage and the reason the engine choice gates
  the number.

### Targets as done-conditions (not dates)

Per house convention (targets over time estimates):

- **T-static:** reftest runner green on a chosen `css/CSS2/` + `css/css-flexbox/`
  subset; pass-rate published per directory; zero `mozjs`/engine deps in the
  graph (audit gate).
- **T-prerender:** the rakers fixture corpus (SSR/SPA initial renders) reserializes
  to non-empty `<body>` for the top frameworks; run against *both* quickjs and boa
  backends to seed the engine-attribution baseline.
- **T-scripted-core:** testharness runner boots `serval-scripted`; `dom/nodes/`
  and `dom/events/` testharness suites run to completion (not necessarily pass) on
  the default backend; per-suite pass rates published per backend so the engine
  ceiling is visible.
- **T-scripted-breadth:** named `dom/`+`html/dom/`+`cssom/` pass-rate targets,
  ratcheted upward; failures triaged into engine-axis vs binding-axis buckets.
- **T-fullweb:** workers/storage/fetch suites attempted; gated on constellation
  re-integration (profile-ladder P6) and a near-spec backend (SpiderMonkey)
  selected for the fullweb build.

The dashboard is two numbers per WPT directory per backend: **attempted%** (did
the harness run) and **passed%**. The gap between backends at equal binding
breadth is the engine tax, made legible.

---

## Part 5 — the modular pattern, generalized to serval's other big parts

The [layout_dom_api doc](./2026-05-16_layout_dom_api_design.md) floated its
ID-first+visitor shape as a **"candidate house pattern."** The engine-swap axis in
this plan is the same move on a *service* boundary rather than a *tree* boundary.
Generalize it. The shared recipe across every big subsystem:

1. An **`*-api` contract crate** — the trait family, no provider deps.
2. **One or more provider crates** — each a concrete impl.
3. **Profile/package witnesses** — a low profile *structurally excludes* heavy
   providers (witness by package; features unify and leak).
4. **Shape by kind:** tree/graph boundary → ID-first core + visitor + foreign-trait
   adapters (layout_dom_api); service boundary → factory/handle trait + per-impl
   lowering (this plan's `ScriptEngine`); query boundary → common-minimum trait +
   engine-specific extension traits (hekate observable planes).

Applied to Serval's big parts:

| Subsystem | Contract crate | Providers | Witness / status |
| --- | --- | --- | --- |
| **DOM** | `layout-dom-api` (exists) | `serval-static-dom` (exists), `serval-scripted-dom` (this plan) | static profile = static-dom only; scripted adds `LayoutDomMut` impl |
| **JS engine** | `script-engine-api` (this plan) | **nova (primary, native-only)** / boa (oracle + **wasm backend**) / quickjs / spidermonkey | nova is 64-bit-bound, native-only (Appendix B); Boa runs the wasm cells; spidermonkey native-only; static/interactive exclude all |
| **Paint output** | `paint-api` / `PaintList` (exists; [paintlist doc](./2026-05-17_paintlist_polyglot_renderer.md)) | NetRender (vello), `DrawExternalTexture` escape hatch | common-minimum items + per-engine `*PaintExt` |
| **Layout** | `layout-api` + the planes (exists; [planes doc](./2026-05-17_serval_layout_planes_architecture.md)) | serval-layout = Stylo (style) + Taffy (box) + parley (text) | each plane swappable in principle; Taffy is the box-layout provider |
| **Cross-engine observables** | `engine-observables-api` (proposed, hekate doc) | Nematic / Serval / Scrying lanes | each lane implements the subset it has |
| **Net/loading** | `*-loading-api` (TBD; `components/net` dead-on-disk) | reqwest/hyper/ureq native; a smolweb loader; a wasm `fetch` loader | static = string/file input only; fullweb = full loader |
| **Storage** | `storage-api` (TBD; `components/storage` dead-on-disk) | in-memory, redb, none | scripted = optional; fullweb = yes |

The decision rule when introducing a new big part (lifted and extended from the
layout_dom_api doc):

1. Identity ops on the surface? → opaque IDs.
2. Dominant "walk all" mode? → visitor with default impl over the IDs.
3. Foreign traits in the consumer set (Stylo-shaped)? → adapters at the consumer,
   don't reshape the core.
4. Compact command stream / iterator-y? → just expose a slice; no pattern.
5. **Multiple swappable implementations with different platform reach (the JS-engine
   case)?** → factory/handle trait + per-impl crates + **package witnesses gated
   by target**, so an unavailable provider (SpiderMonkey on wasm) is *absent*, not
   *feature-off*.

Rule 5 is this plan's addition to the house pattern.

### Caveat — don't proliferate contract crates speculatively

The layout_dom_api doc's own warning applies: net/storage providers are
**dead-on-disk today** and have *no live consumer*. Stand up their `*-api` crates
only when the scripted/fullweb tier actually pulls them, not now. The pattern is a
recipe to apply on contact, not a mandate to pre-build seven trait crates. Same
discipline the ladder's P6 states: extract a shared core from *proven* overlap,
not guessed.

---

## Part 6 — Nova primary and arena composition

Decided 2026-05-21: **Nova is the primary backend; Boa is the conformance oracle.**
Not hedge-by-committee — a bet on architectural fit, with Boa kept for a specific job.

**Sequencing decided 2026-05-22 (after Appendix C):** Nova's reflector hook
(`EmbedderObject`) is an unimplemented stub on 1.0, so **build/validate the
`ScriptEngineLive` reflector against Boa first** (its `JsClass` + captures API is
complete today, and Boa needs a reflector impl regardless as the oracle). **Nova
remains the primary *destination***, migrated to as its embedder surface matures
(via the OrdinaryObject interim, then native `EmbedderObject` once upstreamed).
Critically, the trait is designed to fit **both** from day one — Appendix C already
pins Nova's constraints (state via `HostHooks` not captures; non-capturing `fn`
pointers; `Global` rooting), so "Boa-first" validates the design, it does not shape it.

### Why Nova primary

The other backends are *mature engines bolted onto a foreign DOM*. Nova is the only
one whose **core data model is the model we already chose for the DOM**:

- **Arena isomorphism.** `layout-dom` is already an opaque-`NodeId` handle-arena, not
  a pointer graph. Nova's heap is the same shape — every JS value is a 32-bit
  type-discriminated index into a per-kind vector, compacted on GC. The DOM and the JS
  heap are *isomorphic* arenas, so the bridge is index↔index. The bridge to
  SpiderMonkey's pointer-graph C++ heap is the 772-file impedance mismatch. Nova is
  impedance-matched by construction.
- **Rust-native rooting, no FFI, no build env.** No JSAPI, no NASM/clang-cl. The GC
  marriage is between two Rust collectors that both speak `Trace`.
- **Lifetime-enforced rooting may *prevent* the Boa footgun.** Appendix A Finding 1:
  Boa's GC can't see closure captures, so capturing a traceable value is UB unless
  routed through an explicit captures channel. Nova roots via lifetimes + a `Global`
  root set — a `Global` is live because it's *registered*, regardless of where it's
  captured, and an unrooted handle can't escape its `GcScope` (the borrow checker
  rejects it). **Hypothesis (the prototype must confirm): capturing a `Global` in a
  host callback is safe-by-construction in Nova** — the inverse of Boa, where the
  natural path is UB. If it holds, Nova is *more correct* for the binding layer, not
  just prettier.
- **wasm-capable** — pure Rust, no JIT, Vec-GC; needs one small upstream patch
  (gate the unconditional `usdt` dep) + a feature trim, not a rewrite (Appendix B).

Honest costs, eyes open (the Xilem deal, knowingly taken): ~80% test262 (grind, not
rethink); "acceptable, not fast" perf with optimization explicitly deferred upstream;
**1.0 ≠ frozen API** — upstream plans frequent breaking majors and no LTS, so our
binding layer tracks churn; and the embedder/exotic-object API (our reflectors) is
under-documented — the #1 thing to prototype.

### Boa as conformance oracle (not just a spare tire)

Keep `script-engine-boa` compiled in. Run the **same bindings** against Boa's ~94%
test262 to bucket any WPT/test262 failure instantly: passes on Boa but fails on Nova ⇒
Nova spec gap (grind / contribute upstream); fails on both ⇒ our binding bug. The
modular trait turns the more-conformant engine into a debugging instrument. *That* is
the job — primary-vs-oracle, not primary-vs-spare.

### Arena composition — what "compose the two arenas" means

Three levels, increasing unification and cost:

1. **Reflector-composition (adopted).** DOM arena (Serval's) and JS heap (Nova's) stay
   two arenas, bridged by reflectors: each live node's JS reflector is a Nova object
   holding the `NodeId`; the node holds a rooted `Global` back. Both Rust
   handle-arenas, both GC-traced natively, no FFI. Two arenas, *one experience*. The
   reflector→`NodeId` link is stable (NodeIds don't move); the DOM→reflector link is a
   `Global` Nova's moving GC auto-remaps. The cross-heap cycle lives entirely in Nova's
   heap (the reflector traces its handlers) and is collectable by Nova alone.
2. **Shared substrate (speculative).** DOM nodes and reflectors both backed by one
   Serval "house arena" lib (the layout_dom_api candidate pattern), Nova's core heap
   adjacent. More unified, more work; revisit only if level 1's two-arena overhead
   ever measures.
3. **Literal merge (deferred, upstream).** DOM nodes as entries in Nova's heap vectors
   — truly one arena. Needs Nova to expose adding a heap object-kind (not an embedder
   extension point today) **and** makes `layout-dom` depend on `nova_vm`, deleting the
   "DOM without an engine" capability. Don't pay that now; record as a research
   direction contingent on upstream Nova support.

**"JS leaves the arena when Nova isn't compiled in" = level 1 under compile-time
gating.** The DOM arena is always Serval's and always present; the *reflector half* of
a node is what's gated. Compile Nova out (`serval-static-dom`) → reflector field and
the `nova_vm` dep vanish, pure DOM arena remains. Compile it in (`serval-scripted-dom`)
→ nodes gain reflectors. The one correction to the intuition: reflectors are *compiled
out*; nodes don't *migrate out of a shared vector*.

Both gating modes are supported, for different consumers:

- **Compile-time** (witness-by-package): the true minimal build, no engine in the
  binary. Serves embedded/library use and the smallest wasm bundles.
- **Runtime** (one app binary): link Nova, create a realm *lazily per-document*; a node
  carries `Option<Reflector>`, populated only when a page needs JS. The
  one-binary-mixed-tabs browser case. Viable because the reflector is *additive*.

### The target/capability re-axing (why the tiers change shape)

The static/interactive tiers were `mozjs`-free **because SpiderMonkey can't compile to
wasm** — "the wasm-safe profile = the no-JS profile" was a SpiderMonkey-shaped
constraint. With Nova (whole stack, DOM + JS, wasm-capable — pending the small usdt
upstream gate, Appendix B), **wasm stops being a
capability tier and becomes a build target selectable for any tier.** The model
re-axes from a ladder that conflated "low tier" with "wasm-safe" into an orthogonal
matrix:

| capability | native | wasm |
| --- | --- | --- |
| **none** (DOM-only) | ✓ | ✓ (tiny bundle) |
| **scripted** (DOM + Nova) | ✓ | ✓ |
| **fullweb** | ✓ | ✓ (workers/storage permitting) |

"Whole stack everywhere" is true in the sense that *any capability is buildable for any
target* — **not** that every build links everything.

The no-JS tier does **not** collapse — only its *wasm* justification did. It survives
on three target-independent grounds:

- **Attack surface** — rendering trusted/reader content with zero script execution in
  the process is a security property, not a wasm property.
- **Bundle size — and the counterintuitive part: omit-JS matters *most* in wasm.** A
  reader-mode PWA should ship a tiny DOM-only wasm bundle, not Nova's heap + GC. wasm
  makes capability-tiering *more* valuable even as it kills wasm-as-a-tier.
- **DOM-as-a-library** — extract / serialize / `querySelector` / reader consumers never
  instantiate an engine; the `layout-dom` seam serves them on any target.

So: adopt one arena as the *default app composition* (level 1, runtime, Nova linked),
**and keep** the compile-time drop-to-DOM-only witness — precisely because wasm makes
that option more useful, not less.

> **Cross-doc reconciliation needed.** This re-axing revises the wasm-safety *rationale*
> in the [profile-ladder plan](./2026-05-12_serval_profile_ladder_plan.md) and the
> audit-canary wording in the [hekate lanes doc](./2026-05-17_hekate_lanes_observables.md)
> ("static/interactive must stay mozjs-free"). Those still hold *as written* — mozjs-free
> is still correct, SpiderMonkey is still native-only — but the *reason* shifts from
> "wasm-safety" to "attack-surface + bundle-size + DOM-as-library." Reconcile those docs
> in a follow-up pass rather than letting this one silently contradict them.

### Contingency — `nova_vm` → wasm32: FAILED (the wasm leg uses Boa, not Nova)

The re-axing's wasm leg assumed `nova_vm` compiles to wasm32. **It does not** (empirical,
2026-05-22 — [Appendix B](#appendix-b--wasm32-verification-2026-05-21)). Surmounting usdt
(gated), `ecmascript_atomics` (feature-dropped), and getrandom (configured) exposed the
real wall: `nova_vm`'s own code asserts `size_of::<Value>() == size_of::<usize>()` and
shifts by 53 bits — it is **64-bit-bound by design**, and wasm32 has 32-bit `usize`. Core
upstream work against Nova's DOD premise; not a gate, not soon.

**Consequence:** the matrix's wasm scripted/fullweb cells run **Boa** (pure Rust,
32-bit-clean, already in browsers), selected via the `ScriptEngine` trait — Nova native,
Boa wasm. The re-axing *holds* (any capability buildable for any target) but is **not
same-engine-everywhere**. This is precisely what the modular trait is for; the finding
makes it load-bearing rather than theoretical. (`wasm64`/memory64 would likely work but
browsers don't run it in production.)

---

## Validation ladder

Profile/engine dependency-absence gates (the load-bearing audit checks):

```powershell
# static/interactive tiers stay engine-free
cargo tree -p serval-static-html  | rg "script-engine|mozjs|boa_engine|rquickjs|nova"   # expect: no matches
cargo tree -p serval-interactive-html | rg "script-engine|mozjs|boa_engine|rquickjs|nova" # expect: no matches

# wasm scripted graph excludes spidermonkey/mozjs
cargo tree -p serval-scripted --target wasm32-unknown-unknown | rg "mozjs|spidermonkey"   # expect: no matches

# each backend builds in isolation
cargo check -p script-engine-quickjs
cargo check -p script-engine-boa
cargo check -p serval-prerender --features quickjs
cargo check -p serval-prerender --features boa
```

Behavior gates:

```powershell
# prerender settles a known SPA fixture to non-empty body, both backends
cargo test -p serval-prerender --features quickjs -- ssr_fixtures
cargo test -p serval-prerender --features boa     -- ssr_fixtures

# WPT runners (once they exist)
cargo run -p wpt-reftest -- css/CSS2          # static tier, no engine
cargo run -p wpt-testharness --backend quickjs -- dom/nodes
cargo run -p wpt-testharness --backend boa     -- dom/nodes   # engine-attribution delta
```

---

## Pitfalls

- **Treating prerender as live scripting.** It runs JS once against a faked DOM.
  No interaction, no post-settle mutation. Selling it as more is the trap; it's an
  on-ramp, not the destination.
- **Engine ceiling laundering.** Reporting one WPT number hides whether a failure
  is the engine or the bindings. Always attribute against test262/backend-delta.
- **Feature-unification leak.** A `script-engine-*` crate pulled transitively into
  a static-tier build by a default feature would silently re-arm `mozjs`. Witness
  by package; the `cargo tree | rg` gates are not optional.
- **Re-marrying the DOM to one engine.** The whole point of the cut is the
  engine-neutral `layout-dom` seam. Any binding code that bakes JSAPI/boa types
  into `layout-dom-api` or `serval-scripted-dom`'s *public* surface re-creates the
  772-file problem. Engine types live behind the `script-engine-*` impls (the
  `new_function` lowering) only — never in `layout-dom-api`, `serval-scripted-dom`,
  or `script-engine-api` public types.
- **Treating the VM interrupt as a sandbox.** `set_deadline` bounds runtime, not
  capability. Remote prerender/scripted JS must run with an explicit allow-listed
  capability set in a worker/process/wasm sandbox; the in-process path is for
  trusted fixtures only. A high opcode budget on hostile JS next to local-first
  state is not containment.
- **Building net/storage contract crates before a consumer.** Dead-on-disk
  components have no live consumer; stand up their APIs on contact.
- **Letting prerender's reserialize-and-reparse double-parse become the steady
  state for live pages.** Prerender's "JS string → reparse → StaticDocument" is
  fine for snapshots; live scripting must mutate the real DOM in place via
  `LayoutDomMut`, not round-trip through HTML strings.

---

## Open questions

1. ~~**VM primitive surface** — does `new_function`'s captured-state + GC path
   express across rquickjs and boa?~~ **Answered by the Appendix A paper probe
   (2026-05-20).** It holds, with two consequences: `new_function` mandates
   *explicit* per-backend-bounded captures (`EngineCaptures`: rquickjs blanket,
   boa `Trace`), and live-DOM GC needs a `ScriptEngineLive` extension with
   `Reflector`/`Rooted` associated types — not plain `new_function`. A *compiling*
   two-backend prototype before the trait is frozen is the remaining gate (the
   layout_dom_api "ship the impl with the trait" rule); the paper probe only
   confirms the shape is possible.
2. **Microtask/event-loop ownership for live scripting.** Prerender pumps to
   settle. Live scripting needs a real event loop (timers, rAF, Promise jobs,
   input-driven callbacks) integrated with the host's frame loop and the planes
   architecture's invalidation. It lives in the `script-runtime-api` layer (built
   on the VM's `pump_microtasks`), not in `script-engine-api`. Confirm whether the
   event-loop *driver* sits in `script-runtime-api` itself or in `serval-scripted`
   which owns the host frame loop.
3. ~~**Backend default per target.**~~ **Resolved 2026-05-21 (Part 6):** Nova is
   primary on every target; Boa is compiled in as the conformance oracle; quickjs is
   an optional alt; SpiderMonkey is native-fullweb-only, if ever. The trait keeps it
   reversible.
4. **Does prerender warrant its own profile-ladder tier** (`serval-prerender`
   between interactive and scripted), or is it a *mode* of `serval-scripted` that
   omits `LayoutDomMut`? Lean: own crate, since its dependency graph
   (engine + static-dom, no mutation) is a genuine witness distinct from scripted.
5. **WPT vendoring.** Pinned upstream WPT checkout vs a curated subset in-repo?
   Affects the runner crates' shape. Likely a pinned submodule + a manifest of
   which directories each profile attempts.
6. ~~**Nova adoption unknowns.**~~ **Probed 2026-05-21/22** (Appendices B & C):
   (a) the `Global`-capture-safety hypothesis is **moot** — Nova native fns are
   non-capturing `fn` pointers; state flows via `Agent`/`HostHooks`, so there's no
   capture footgun to begin with; (b) the reflector API is **not under-documented, it's
   unimplemented** — `EmbedderObject` is a `todo!()` stub and the trace hook is
   crate-private, so the clean reflector is upstream feature work (interim:
   `OrdinaryObject`-as-reflector); (c) wasm32 **needs a small upstream `usdt` gate**, not
   green out of the box. Net: the open question is now a **sequencing decision** (Nova
   upstream-first vs OrdinaryObject-interim vs Boa-first-to-build-against), not a
   feasibility unknown. Literal single-arena (Part 6, level 3) stays deferred upstream.

---

## Exit criteria — when this plan is wrong

- If the VM primitive surface (esp. `new_function` + captured-state GC integration)
  can't be expressed across rquickjs + boa without a per-backend escape hatch bigger
  than the shared trait, the abstraction isn't paying for itself — collapse to
  per-backend `serval-scripted-{quickjs,boa}` crates with no shared trait (rakers'
  own answer at its scale).
- If prerender's reserialize-and-reparse loses too much fidelity on the target
  framework corpus (event handlers, hydration markers stripped), prerender isn't a
  viable lane and the on-ramp is straight to live scripting.
- If the engine-attribution dashboard shows the binding gap dominates the engine
  gap at every directory, the modular-engine investment is academic for coverage
  purposes (still useful for wasm reach) — prioritize bindings over backends.

---

## Decision log

- **Proposed 2026-05-20:** scripting is added as a **swappable backend**
  (`ScriptEngine` trait + per-engine provider crates), not a re-import of
  `components/script`. The 2026-05-15 cut's `layout-dom` seam is the plug point.
- **Proposed 2026-05-20:** three execution modes — static (no engine), **prerender**
  (one heap, rakers model, no bindings), live scripted (two heaps, `LayoutDomMut`
  bridge) — mapped onto the existing profile ladder. Prerender is the new on-ramp.
- **Proposed 2026-05-20:** `ScriptEngine` is distinct from and *below*
  `ScriptThreadFactory`; the fullweb factory is built atop a backend, deferred to
  ladder P4/P6.
- **Proposed 2026-05-20:** resolves layout_dom_api open question #1 — DOM mutation
  is a `LayoutDomMut: LayoutDom` extension trait on a new `serval-scripted-dom`
  provider. The provider emits render-state-free `DomMutation` records (no
  `mark_dirty`); serval-layout's scheduler translates them into invalidation. The
  live GC marriage lives in each backend's reflector (`ScriptEngineLive`, per
  Appendix A), not in plain `new_function` and not in any public DOM/engine type.
- **Proposed 2026-05-20:** WPT coverage is tracked on **two axes** — engine
  conformance (test262 as a *diagnostic baseline*, fixed per backend) and binding +
  event-loop + platform conformance (WPT, Serval's work). No `min()` formula:
  test262 is not a literal WPT cap; event-loop/module/timer/URL/fetch/harness
  behavior dominates `dom/`/`html/` regardless of language score. Triage failures
  into engine-language / host-event-loop / binding-breadth buckets; cross-backend
  deltas confirm the engine-language attribution.
- **Proposed 2026-05-20:** the layout_dom_api "candidate house pattern" is extended
  with **rule 5** (swappable implementations with differing platform reach →
  factory trait + per-impl crates + target-gated package witnesses) and applied to
  DOM / JS engine / paint / layout / observables / net / storage. Net & storage
  contract crates deferred until a live consumer exists.

Review-pass corrections (2026-05-20):

- **`script-engine-api` is VM-only.** Browser host surface (document, timers,
  fetch, bootstrap, fake/real DOM, event loop) moved up to a `script-runtime-api`
  layer built on the VM primitives. If a browser concept appears in
  `script-engine-api`, the boundary has failed.
- **The VM trait is object-unsafe** (associated `Value`/`Error`, `Self`-typed
  callbacks) and monomorphizes per backend. Runtime A/B for the WPT runner goes
  through an explicit `Backend` dispatch enum, not `dyn ScriptEngine`. The doc no
  longer implies free runtime polymorphism.
- **Prerender sandbox is mandatory for remote content.** Hostile remote JS runs in
  a constrained worker/process/wasm sandbox with an allow-listed capability set
  (timers + document shims + policy-gated brokered fetch; nothing else). A VM
  deadline is not a trust boundary. In-process prerender is a trusted-fixture
  affordance only.
- **Prerender is a Serval render-tier selection, not a Hekate E3 extraction mode.**
  Hekate's app-shell/empty-root signals are tier-selection *evidence* passed as a
  hint; Serval owns the tier decision. Keeps the E0–E4 extraction vocabulary clean.
- **Engine API paper probe (Appendix A) confirms the layered trait** against
  rquickjs 0.11 + boa 0.21. Three durable consequences: `new_function` takes
  *explicit* captures bounded by a per-backend `EngineCaptures` trait (boa can't
  trace closure environments); live-DOM GC needs a `ScriptEngineLive` extension with
  `Reflector`/`Rooted` types (re-deriving the reflector pattern per-backend); boa has
  no preemption, so its sandbox must be an externally-killable process. A compiling
  two-backend prototype is the remaining gate before the trait freezes.

Direction set (2026-05-21, Part 6):

- **Nova is the primary backend; Boa is the conformance oracle.** The bet is
  architectural fit: Nova's handle-arena heap is *isomorphic* to `layout-dom`'s
  `NodeId` arena, so the JS↔DOM bridge is index↔index and Rust-native (no FFI, no
  JSAPI, no build env) — impedance-matched where SpiderMonkey is the 772-file
  mismatch. Boa stays compiled in to bucket failures (Nova-spec-gap vs binding-bug).
  Costs taken knowingly: ~80% test262, "not fast" perf, and a deliberately
  unstable 1.0 API (frequent breaking majors, no LTS) — the Xilem deal.
- **Arena composition = reflector-composition (level 1).** Two cooperating Rust
  handle-arenas bridged by reflectors; *one experience*, not a literal merge.
  Compile-time gating drops to DOM-only (no engine in the binary); runtime gating
  creates realms lazily per-document (one app binary, mixed tabs). Literal
  single-arena (DOM-in-Nova's-heap) deferred as an upstream research item — it would
  delete the "DOM without an engine" capability.
- **Tiers re-axed: capability × target matrix, but per-target engine.** wasm stops being
  a capability tier (it was a SpiderMonkey-shaped constraint); capability
  (none/scripted/fullweb) × target (native/wasm) is a matrix. The no-JS tier survives on
  attack-surface + bundle-size + DOM-as-library grounds (omit-JS matters *most* in wasm).
  **But the wasm scripted/fullweb cells run Boa, not Nova** — the empirical wasm build
  (2026-05-22, Appendix B) found `nova_vm` is **64-bit-bound** (`Value` is `usize`-sized;
  `usize` is 32-bit on wasm32) and does **not** compile to wasm32. Nova is native-only;
  the `ScriptEngine` trait does per-target backend selection (Nova native / Boa wasm).
  Cross-doc reconciliation (profile-ladder + hekate docs) **done 2026-05-21**.
- **Two-track sequencing decided 2026-05-22** (Appendix C surfaced that Nova's
  `EmbedderObject` reflector hook is an unimplemented stub). **Track A — Boa-first:**
  build/validate the `ScriptEngineLive` reflector against Boa's complete `JsClass` +
  captures API (Boa needs a reflector impl anyway as oracle); this validates the trait
  design. **Track B — patch Nova** via `[patch.crates-io]` → a thin `serval-embedder` branch on
  `github.com/mark-ik/nova` adding a `NodeId` slot to `EmbedderObject` + `InternalSlots`
  delegation to its backing object (+ the usdt wasm gate); **no trace-hook surgery**
  (handlers live on the backing object Nova already traces). Build `script-engine-nova`
  against the patch; upstream the diff so the branch disappears. Not a heavy fork. Both tracks share `ScriptEngineLive`;
  Nova remains the primary *destination*. The trait is designed to fit both from day one
  (Appendix C pins Nova's constraints), so Boa-first validates the design without shaping
  it. OrdinaryObject interim demoted to optional smoke-test.
- **Track A built and green (2026-05-22)** at `probes/script-engine-probe/` (standalone
  workspace, isolated from serval's audit gates). `ScriptEngine` + `ScriptEngineLive`
  implemented against **Boa 0.21**; three passing tests round-trip a `NodeId`-carrying
  native reflector read back in a native callback (**Finding 2**), JS mutating Rust host
  state via a `Trace`-payload `from_copy_closure_with_captures` (**Finding 1**), and a
  forced-GC pass over a live reflector. **Validated:** the trait shape is implementable
  with engine types fully confined to the backend; both Appendix A findings compile and
  run on a real engine. **Cross-heap rooting now exercised too** (4th test): a host-held
  JS handler keyed by `NodeId` survives a forced GC — but *only* because it lives in a
  Boa-**traced** `HostDefined` table. Finding: **Boa has no free-standing persistent
  root** (unlike rquickjs `Persistent` / Nova `Global`); a `JsObject` parked in untraced
  Rust would be collected, so the host must thread handlers through Boa's traced graph.
  That is the baseline the Nova `Global`-capture-safety hypothesis (Track B) is measured
  against — if Nova lets a `Global` handler sit in the plain-Rust DOM arena across GC,
  it's the cleaner model, as bet.
- **Track B1 green — hypothesis CONFIRMED (2026-05-22)** at `probes/nova-probe/`
  (path-deps the local fork clone at `repos/nova`). A JS string reachable **only**
  through a `Global` held in a plain-Rust `Option` survives forced `agent.gc()` passes
  unchanged. `Global`'s own source confirms it: a detached root (an index into
  `agent.heap.globals`), explicitly released — exactly the free-standing root Boa
  lacks. **So the architectural bet holds at the rooting layer: the Rust DOM arena can
  hold `Global` handlers directly, no traced-container dance.** Secondary findings:
  (a) Nova surfaces script completion values (`eval "1 + 2"` → `"3"`; an earlier
  `undefined` was the *directive-prologue* rule, not a gap); (b) values are gc-scoped —
  the embedder holds `Global`s, not bare values, and feels the `bind`/`unbind` +
  `gc.nogc()`/`reborrow()` discipline (real but, on this surface, a handful of calls,
  not pervasive); (c) **`call_function`/`is_callable` are `pub(crate)`** — the public
  API can't invoke a held function; you drive JS via `parse_script`/`script_evaluation`
  and builtins. (c) is a binding-layer constraint and a candidate for the same upstream
  patch as the reflector.
- **Track B2 patch scope pinned from source (2026-05-22).** Read
  `nova_vm/src/ecmascript/builtins/embedder_object{.rs,/data.rs}`: all `InternalSlots`
  methods are `todo!()` over `EmbedderObjectHeapData { backing_object: Option<OrdinaryObject> }`.
  The `InternalSlots` trait *defaults* `internal_extensible`/`internal_prototype`/etc.
  through `get_or_create_backing_object`, so the patch is even thinner than Appendix C
  implied: **(1)** add `embedder_data: u64` to the heap data (plain integer — carries
  the `NodeId`; no trace, just ignore it in mark/sweep); **(2)** implement the three
  backing-object methods (`get`/`set`/`create_backing_object`) over the arena, and
  **delete** the four `todo!()` extensible/prototype overrides so the trait defaults run;
  **(3)** add `pub fn create_with_data(...)` + `pub fn embedder_data(self, &Agent) -> u64`;
  **(4)** the `usdt` wasm target-gate (Appendix B). ~1 field + 3 small method bodies +
  2 pub fns. Apply on the `serval-embedder` branch; consume via `[patch.crates-io]`.
- **Track B2 patch landed and green (2026-05-22).** The patch (above) is implemented on
  branch `serval-embedder` of `repos/nova` (commit `fbca54b`): `embedder_data: u64` slot,
  the three backing-object methods + `internal_prototype`, deleted the four `todo!()`
  overrides, `create_with_data`/`embedder_data` accessors. `cargo build -p nova_vm` is
  clean (first try — the Error backing-object template held). `probes/nova-probe/`'s
  `embedder_object_native_data_survives_gc` is green: a JS-visible `EmbedderObject`
  carrying a `NodeId` as native data, rooted via a plain-Rust `Global`, survives forced
  GCs and reads its `NodeId` back. **Finding 2 validated on Nova — and the full bet now
  holds end-to-end:** native-data reflector + `Global` rooting from the plain-Rust DOM
  arena, no traced-container dance. Branch **pushed to `github.com/mark-ik/nova`**
  (`serval-embedder`); upstream PR to trynova/nova is the remaining step. (No `usdt` wasm
  gate in this branch yet — orthogonal; separate commit.)
- **Part 3 — probes lifted to real serval crates (2026-05-23).** Three crates under
  `components/`: **`script-engine-api`** (the trait crate — `ScriptEngine` +
  `ScriptEngineLive` with `make_reflector`/`reflector_data`; DOM-neutral, no engine dep);
  **`script-engine-nova`** (native-only via `cfg(not(wasm32))`, `nova_vm` redirected to the
  fork clone by a root `[patch.crates-io]`); **`script-engine-boa`**. All three —
  api, nova, **and now boa** — are serval **workspace members and green**
  (`script_engine_nova` reflector round-trip survives GC in-workspace).
- **RESOLVED — Boa joined serval's graph via a fork (2026-05-25).** *Prior blocker
  (2026-05-23):* `boa_engine 0.21.1` hard-deps `icu_normalizer ~2.0.0` (non-optional);
  serval's **parley 0.9** needs `^2.1.1` — disjoint, so adding boa broke workspace
  resolution. (Worse, in the live graph nova_vm → `temporal_rs 0.2.3` → `icu_calendar
  2.2.1` force-pins the whole icu family to **2.2.x**, so the real target was 2.2, not
  2.1.) *Resolution:* took option **(c) fork/patch boa's icu** — which the 2026-05-23
  note feared as "heavy" and **it was the opposite of heavy.** Forked boa to
  `crates/boa` (`serval` branch, shallow @ `v0.21.1`/`bc36c3f`), widened the icu family
  pin `~2.0` → `^2.1` (one workspace `Cargo.toml` edit), and redirected `boa_engine` +
  `boa_gc` via serval's root `[patch.crates-io]` (same pattern as `nova_vm`).
  **boa_engine compiled clean against icu 2.2.0 with zero code changes** — the `~2.0.0`
  pin was precautionary tilde-caution, not a real API incompatibility. `cargo check -p
  script-engine-boa` is green in the unified workspace; `icu_normalizer` resolves to a
  single 2.2.0 shared by boa, parley, and nova. (One benign deprecation warning in boa's
  `host_defined.rs`: `hashbrown::get_many_mut` → `get_disjoint_mut`; cosmetic, upstream's
  code.) This unblocks boa in **both** roles — native conformance oracle *and* the wasm
  backend — and, more importantly, gives us an **owned boa fork to restructure for
  weval-based AOT** (the actual reason the pin "stopped being an excuse": we have to fork
  to weval-ify the interpreter anyway, so the icu bump is free along the way). The earlier
  "wait for upstream / no fork" decision is **superseded**; the quickjs-pivot option is moot.
- **serval-scripted-dom — foundation built + green (2026-05-23).** Resolves
  `layout-dom-api`'s open question #1: added **`LayoutDomMut`** (`create_element`,
  `create_text`, `append_child`, `remove`, `set_attribute`, `set_text`,
  `drain_mutations`) + the render-state-free **`DomMutation`** enum
  (`Inserted`/`Removed`/`AttributeChanged`/`CharacterDataChanged`). New
  `components/serval-scripted-dom` (workspace member): a mutable `NodeId` arena
  implementing both `LayoutDom` (read) and `LayoutDomMut`, recording each structural
  change as a `DomMutation` and draining the stream. Two tests green (mutate/read/record;
  siblings/remove). The arena owns node data; JS reflectors will bridge back by `NodeId`.
  **Next pass (flagged, not done):** (1) `set_inner_html` (needs html5ever fragment
  parsing); (2) the **reflector bridge** wiring (NodeId ↔ `make_reflector`/`reflector_data`)
  through the `script-runtime-api` host layer; (3) the **`DomMutation` → serval-layout**
  invalidation loop (serval-layout's scheduler consuming the drained stream). The probes
  validated the engine side; (2)+(3) close the engine↔DOM↔layout loop.
- **Reflector bridge green — JS→DOM works end-to-end (2026-05-23).** New
  `components/serval-scripted` (workspace member, native-only): a JS script handed a
  `node` **reflector** (an `EmbedderObject` carrying the div's `NodeId`) calls a native
  `setText(node, …)`; the callback recovers the `NodeId` via the reflector, reaches the
  host `ScriptedDom`, and calls `LayoutDomMut::set_text` — the DOM mutates **and** records
  a `DomMutation::CharacterDataChanged`. Test `js_mutates_dom_through_reflector` is green.
  **This is the binding layer (the "hard 90%") demonstrated end-to-end**, leak-free: the
  reflector carries only a `NodeId`, the DOM stays in Rust, the engine never owns DOM data.
  Probe-grade caveats (follow-up): host DOM reached via `thread_local` (rakers pattern,
  not Nova host-data); bindings installed at realm-init via a plain `fn`; uses `nova_vm`
  directly rather than layering cleanly on `script-engine-nova` / a `script-runtime-api`
  host layer. Next: **#2** `DomMutation` → serval-layout invalidation (assess the
  scheduler seam first), then **#3** `set_inner_html`.
- **#2 assessed (2026-05-23): no incremental seam — it's a subsystem, not a wire.**
  serval-layout is generic over `D: LayoutDom` (so it *can* lay out a `ScriptedDom`), but
  runs a **full** pass with Stylo's restyle machinery (`set_dirty_descendants`,
  `compute_layout_damage`) **stubbed to no-ops** — no incremental invalidation exists.
  So #2 splits: **(a) coarse** — `DomMutation` non-empty → re-run the full
  construct→cascade→layout→paint pipeline over the mutated DOM (closes the loop; a real
  integration needing serval-layout's font/viewport/pipeline setup; not a quick wire);
  **(b) incremental** — mark-dirty → relayout affected subtrees only (the planes doc's
  design-stage scheduler; substantial new subsystem). The `DomMutation` stream is the
  correct relayout signal either way — the API shape is right. Recommendation: do **#3**
  (self-contained) next; treat #2 as a focused layout-integration session, starting with
  (a) coarse relayout-on-mutation, (b) incremental later.
- **#3 done — `set_inner_html` green (2026-05-23).** Added `set_inner_html` to
  `LayoutDomMut` + restored the `SubtreeReplaced` `DomMutation`. Implemented in
  serval-scripted-dom by reusing serval-static-dom's parser: parse the fragment into a
  `StaticDocument` (itself a `LayoutDom`), copy its `<body>` children into the mutable
  arena silently, record one `SubtreeReplaced` — no second `TreeSink` needed. Test green
  (`<p>hi</p><span>x</span>` → 2 element children + text). Simplification: uses
  `parse_document` + the body subtree rather than true context-aware fragment parsing
  (fine for the common element-fragment case; TODO: `parse_fragment` with context element).
- **#2 direction (2026-05-23): chose (b) incremental, with (a) coarse as the
  differential-testing oracle.** Decided after the (a)-vs-(b) contrast: (b) is the right
  destination (the planes were designed for invalidation; coarse is pathological for live
  scripting; (b) leans on Stylo's *existing* invalidation that serval merely stubbed). The
  hard part of (b) is correctness (the stale-layout bug class), so build (a) cheaply as the
  ground-truth oracle and **diff incremental against full-recompute** in tests. #2 is its
  own focused session (un-stub Stylo invalidation → `RestyleDamage` → Taffy partial relayout
  → partial paint), starting with the (a) oracle. Awaiting go.
- **#2(a) coarse relayout oracle — DONE, green (2026-05-23).** Added `serval_layout::render`
  (a viewport-typed wrapper over cascade→layout, generic over any `LayoutDom`) and
  `serval_scripted::relayout_if_dirty` (drains the DOM's `DomMutation`s; if non-empty,
  re-runs the full pipeline). Test `coarse_relayout_reflects_mutation`: build a
  `ScriptedDom` (html>body>p), lay out, `set_inner_html(body, "<p>one</p><p>two</p><p>three</p>")`,
  relayout → the three paragraphs **stack vertically** (strictly increasing `location.y`),
  and a no-mutation call returns `None` (gating works). **The whole DOM→layout loop runs
  over a `ScriptedDom`** — serval-layout is generic over `LayoutDom`, no font setup needed.
  One real constraint surfaced + fixed: serval-layout's Stylo style-sharing cache asserts
  `NodeId` is pointer-sized, so the scripted `NodeId` is now `usize`-backed (was `u32`).
- **#2(b) incremental — assessed as a major subsystem; recommend a focused arc, not a tail
  task (2026-05-23).** It is *not* a wire or a single function: serval-layout currently
  *rebuilds everything every pass* (`construct` builds a fresh Taffy tree; `run_cascade`
  does a full cascade; Stylo's restyle/snapshot/invalidation machinery is **stubbed to
  no-ops**). True incremental requires reworking that into: (1) a **damage classifier**
  (`DomMutation` → restyle/relayout/repaint scope: `class`/`style` attr → restyle element +
  selector-reach; structural → sibling-selector + subtree; text → containing-block relayout);
  (2) **un-stubbing Stylo's invalidation** (snapshots + invalidation map + the restyle
  traversal serval no-op'd) for partial restyle; (3) **incremental `construct`/Taffy**
  (cache the tree, `mark_dirty` only affected nodes — serval-layout has no partial-layout
  entry today); (4) partial paint. Each touches serval-layout's core. The **diff-test
  harness is the first scaffolding**: run (a) coarse + (b) incremental on the same mutation,
  assert identical fragments (catches the stale-layout bug class). Recommendation: tackle
  (b) as its own focused arc; the (a) oracle is now in place to validate it against.
- **#2(b) core BUILT + diff-tested (2026-05-24).** The incremental pipeline exists and
  is green, validated against the (a) oracle:
  - **Plan** — `serval_layout::classify(DomMutation) → Invalidation{RestyleSubtree |
    RelayoutSubtree | RepaintNode}` + `coalesce(&[Invalidation], parent_of)` → minimal
    roots (ancestor subsumption with strength ordering; a weaker ancestor never subsumes
    a stronger descendant).
  - **Execute** — `serval_layout::SubtreeView` re-roots a `LayoutDom` so the existing
    pipeline lays out just one subtree; `render_subtree` wraps it.
  - **Splice** — `serval_scripted::relayout_incremental(dom, prior, …)` drains → classifies
    → coalesces → lays out each root's subtree → splices the fragments into the prior plane
    at the root's real position, with a **correct coarse fallback** when a subtree's outer
    size changes (ancestors would reflow) or the root wasn't laid out before.
  - **Diff-tests** (the stale-layout-bug guard): scoped subtree layout matches the coarse
    oracle's *relative* interior geometry; `relayout_incremental` matches the coarse oracle
    at *absolute* positions for a size-stable mutation.
  - **Deferred boundaries** (bounded, documented, all safe — they defer to the correct
    coarse path or only affect removed/inherited cases): inheritance-context threading
    (the `SubtreeView` boundary — scoped cascade uses default inherited style); stale-fragment
    eviction for removed nodes (`SubtreeReplaced` doesn't carry old children); fine-grained
    sub-subtree restyle (un-stubbing Stylo's invalidation map — the optimization); and
    size-change propagation (currently the coarse fallback rather than incremental).
  So incremental went from "design-stage subsystem" to a **working, oracle-validated core**;
  what remains are optimizations and the inheritance/eviction edges, not the mechanism.
- **Fine-grained restyle — investigated; it's the deliberately-stubbed Stylo invalidation
  pipeline → a focused Stylo arc (plan below, 2026-05-24).** Grounded in the source:
  `cascade.rs` builds a real Stylo `Stylist` (UA + author sheets, flushed — its invalidation
  map *is* built but unused) and re-cascades the whole tree each pass; the incremental
  pieces are stubbed — empty `SnapshotMap::new()`, no-op `set_dirty_descendants` /
  `unset_dirty_descendants` / `has_dirty_descendants` (`adapter_stylo.rs`), stub
  `compute_layout_damage`. Faithfully un-stubbing this is Stylo's real incremental-restyle
  path — a multi-session effort that can't meet the diff-tested-correct bar if rushed.
  **Precise plan (execute in a focused Stylo session):**
  1. **Snapshots** — before a mutation, capture the element's old attrs/classes/state into
     the `SnapshotMap` (hook serval-scripted-dom's `set_attribute`/structural mutators);
     implement Stylo's `ElementSnapshot` on the DOM adapter.
  2. **Invalidation map** — query the `Stylist`'s already-built map for the selector
     dependencies of the changed class/attr/id.
  3. **Invalidator** — run Stylo's `StateAndAttrInvalidationProcessor` + `TreeStyleInvalidator`
     over (snapshot, map) to mark the *actually-affected* elements (un-stub the dirty bits).
  4. **Restyle traversal** — re-cascade only the marked elements (vs the current full walk).
  5. **RestyleDamage** — un-stub `compute_layout_damage` (old vs new `ComputedValues` →
     REPAINT vs RELAYOUT) so repaint-only changes skip layout.
  6. **Wire into `relayout_incremental`** — replace `RestyleSubtree`'s whole-subtree
     re-cascade with the invalidation-driven minimal restyle; the coarse-oracle diff-test is
     already in place to validate it.
  **Tractable adjacent alternative (relayout axis, not restyle):** a cascade-skip for
  layout-only (text) changes — `RelayoutSubtree` reuses the prior `StylePlane` and runs a
  layout-only subtree pass, skipping the cascade. Nearer reach, but has its own edges
  (text-node → containing-element resolution; multi-call style staleness) and is a different
  axis than the invalidation-map restyle asked for.

---

## Appendix A — engine API paper probe (2026-05-20)

Paper probe (docs-read, **not** a compiling prototype) against the *current*
crates, not rakers' pins: **rquickjs 0.11.0** (rakers: 0.8) and **boa_engine
0.21.1** (rakers: 0.19). Per the workspace-pins rule these are the versions to pin
a real prototype against; the skew from rakers' vintage is large enough that
rakers' code is a shape reference, not a copy source. Question probed: does
`new_function` + captured-state + GC integration express across both engines, or
does it need an adapter (à la the layout_dom_api Stylo probe)?

### Mapping the minimal VM surface

| `ScriptEngine` method | rquickjs 0.11 | boa 0.21 | holds? |
| --- | --- | --- | --- |
| `eval` (script/module, sloppy) | `Ctx::eval_with_options(src, EvalOptions{ strict, global, .. })`; `eval_promise` for top-level await | `Source::from_bytes` + `Script`/`Module`; `Context::eval` | ✓ |
| value conversion | `FromJs`/`IntoJs` traits | `JsValue` + `TryFromJs`/`TryIntoJs` | ✓ |
| `set_global` | `Ctx::globals() -> Object`, `.set(k,v)` | `Context::register_global_property` / global set | ✓ |
| `pump_microtasks` | `Ctx::execute_pending_job() -> bool` (loop to fixpoint); `Runtime::is_job_pending` | `job::JobExecutor` (`SimpleJobExecutor` = FIFO-to-completion); 0.21 also has a native `TimeoutJob` | ✓ (shape differs: pump method vs installed executor) |
| `set_deadline` | `Runtime::set_interrupt_handler(Option<Box<dyn FnMut()->bool + Send>>)` — cooperative abort | **none** (no fuel/interrupt in 0.21.1) | **asymmetric** |

The minimal VM surface holds on both. Three findings of substance below.

### Finding 1 — `new_function` must mandate *explicit* captures (boa forces it, safely)

The engines diverge exactly at captured state:

- **rquickjs** lets a native function close over Rust state freely (`Func`/`MutFn`/
  `OnceFn` + `IntoJsFunc`); JS values held across calls use `Persistent`; native
  objects trace held JS refs via `Trace`.
- **boa** *cannot* safely let a closure implicitly capture **traceable** (JS)
  state — `boa_gc` can't inspect closure environments, so `from_closure` is
  `unsafe` and UB-prone if captures need tracing. The **safe** path passes captures
  through an explicit, traced channel and keeps the closure `Copy`:

  ```rust
  // boa 0.21 — the GC-safe shape (safe fn).
  pub fn from_copy_closure_with_captures<F, T>(closure: F, captures: T) -> Self
  where F: Fn(&JsValue, &[JsValue], &T, &mut Context) -> JsResult<JsValue> + Copy + 'static,
        T: Trace + 'static;
  // from_closure / from_closure_with_captures exist but are `unsafe` — UB if the
  // closure implicitly captures anything traceable.
  ```

**Consequence for the trait.** `new_function` must *not* expose implicit closure
capture; captures are always an explicit payload bounded by a per-backend trait:

```rust
// ILLUSTRATIVE reshape of new_function.
fn new_function<C: EngineCaptures<Self>>(&mut self, captures: C, f: NativeFn<Self, C>)
    -> Result<Self::Value, Self::Error>;
// EngineCaptures is a per-backend marker: rquickjs blanket-impls it for all
// `'static`; boa impls it only for `T: Trace + 'static`.
```

Harmless on rquickjs (it just doesn't need the discipline), *mandatory* on boa.
Same outcome as the Stylo probe: the trait holds, but it carries a per-backend
bound the original sketch hid.

### Finding 2 — live-DOM reflectors need a `ScriptEngineLive` extension, not just functions

Prerender captures only non-JS Rust state (a `NodeId`, an output sink behind a
Rust handle), so Finding 1's explicit-captures shape covers it entirely —
**prerender needs nothing more.**

Live bindings are different: the DOM↔JS cycle (a node's reflector holds a JS event
handler; the handler captures the node) is only collectable if the **reflector
lives in the JS heap as a traced native object**. Both engines provide this, but
it's a *class* mechanism above `new_function`:

- rquickjs: `Class<'js, T>` + `impl Trace for T` (`#[derive(Trace)]`), `Persistent<T>` for Rust-held JS refs.
- boa: `JsClass`/`NativeObject` + `#[derive(Trace, Finalize)]`, rooted `Gc` handles.

So the scripted tier needs an extension trait carrying associated `Reflector` and
`Rooted` types:

```rust
// Used by serval-scripted-dom only; prerender never sees it.
pub trait ScriptEngineLive: ScriptEngine {
    type Reflector;   // rquickjs Class<T> / boa JsClass instance
    type Rooted<V>;   // rquickjs Persistent / boa rooted Gc
    fn new_reflector(&mut self, node: NodeId, traced: ReflectorTrace<'_>) -> Self::Reflector;
}
```

This **re-derives Servo's reflector pattern** — every live DOM node gets a JS
reflector — but now *per-backend behind the trait*, with the `NodeId`-keyed DOM
store staying in Rust and only the reflector wrapper in the JS heap. This is where
the GC marriage from the design discussion actually lives.

### Finding 3 — boa has no preemption; the sandbox conclusion hardens

rquickjs can abort a runaway script cooperatively (`set_interrupt_handler`). boa
0.21.1 **cannot** — no fuel/interrupt/instruction limit exists. Therefore:

- `set_deadline` → `Cooperative` (rquickjs) / `Unsupported` (boa).
- **For the boa backend, the sandbox MUST be an externally-killable worker/process.**
  There is no in-VM way to bound execution. This upgrades the prerender sandbox rule
  from "constrained sandbox" to "boa on hostile JS in-process is not an option,
  full stop." rquickjs gets cooperative interrupt + process-kill backstop.

### Verdict

The layered trait holds, with one reshape and one extension — no architectural
retreat:

1. Minimal VM surface (eval / value-conv / set_global / pump): **define as-is.**
2. `new_function`: **reshape** to explicit, per-backend-bounded captures (Finding 1).
   Covers the whole prerender surface.
3. Live DOM: **`ScriptEngineLive` extension** with `Reflector` + `Rooted` associated
   types (Finding 2); the GC marriage lives here, parallel to `LayoutDomMut`.
4. `set_deadline` asymmetry **feeds the sandbox design**: boa ⇒ killable process
   mandatory (Finding 3).

Deferred (matching the layout_dom_api stance): a **compiling** two-backend
prototype — `new_function` with explicit captures on both rquickjs 0.11 and boa
0.21, plus one reflector round-trip — before the trait is frozen. The paper probe
confirms we're not designing around an impossible shape; the compile is the proof.

Source pages: rquickjs [function](https://docs.rs/rquickjs/latest/rquickjs/function/index.html) / [class](https://docs.rs/rquickjs/latest/rquickjs/class/index.html) / [Ctx](https://docs.rs/rquickjs/latest/rquickjs/struct.Ctx.html) / [Runtime](https://docs.rs/rquickjs/latest/rquickjs/struct.Runtime.html); boa [NativeFunction](https://docs.rs/boa_engine/latest/boa_engine/native_function/struct.NativeFunction.html) / [class](https://docs.rs/boa_engine/latest/boa_engine/class/index.html) / [job](https://docs.rs/boa_engine/latest/boa_engine/job/index.html).

---

## Appendix B — wasm32 verification (2026-05-21)

Empirical, not paper: built the *published* `nova_vm 1.0` as a downstream dep would,
for `wasm32-unknown-unknown`, on stable cargo 1.92 — five iterations of a throwaway
probe crate. **Verdict: Nova is architecturally wasm-safe (pure Rust, no JIT,
Vec-compaction GC) but does *not* compile to wasm32 out of the box. Two real blockers +
one trivial config, all identified, none structural to Nova's own code.**

### What blocks it

| Blocker | Nature | Fix | Cost |
| --- | --- | --- | --- |
| **`usdt` (DTrace probes)** | **unconditional** dep + build-dep of `nova_vm`; `usdt-impl 0.6` is a flat `compile_error!("USDT only supports x86_64 and ARM64")` with no wasm path in *any* feature config (verified: `usdt` `default-features = false` still fails) | **upstream**: target-gate the dep (`[target.'cfg(not(target_arch="wasm32"))'.dependencies]` + `#[cfg]` the probe sites), or usdt gains a wasm no-op | small, well-defined upstream PR — the load-bearing one |
| **`ecmascript_atomics 0.2`** | `optional`; pulled by the `array-buffer` / `atomics` / `shared-array-buffer` features (all default-on); uses arch-specific inline asm | **consumer-side**: `default-features = false`, omit those features (or upstream a wasm fallback) | loses ArrayBuffer / TypedArray / SharedArrayBuffer JS support until patched |
| **`getrandom` (0.3 *and* 0.4 both in tree)** | bare wasm32 needs an explicit RNG backend | **consumer-side, trivial**: `wasm_js` feature on both majors + `RUSTFLAGS='--cfg getrandom_backend="wasm_js"'` | none |

### What is *not* a problem

Across five build attempts, **no wasm incompatibility surfaced in `nova_vm`'s own code
or in any of its ~230 other deps** (oxc parser stack, temporal_rs, lexical, num-bigint,
ryu-js, wtf8, hashbrown, ahash, dof, chacha20, …). Every failure was one of the three
above. The architecture-fit thesis holds — nothing about Nova's heap/GC/interpreter is
wasm-hostile.

### Honest status & the correction it forces

- **Native:** compiles (Nova's tested target; CI is OS-matrix only, never wasm).
- **wasm32:** **needs a small upstream contribution** — gate `usdt` off non-DTrace
  targets — plus a feature trim (drop `array-buffer`/`atomics` until `ecmascript_atomics`
  gets a wasm fallback) plus the standard getrandom backend. This is *exactly* the
  "provide the grind" the Xilem-style bet signed up for, and an ideal first upstream PR:
  small, mechanical, unlocks wasm for every embedder, lands in a contribution-friendly
  NLnet-funded project.
- **Not yet proven:** a fully *green* wasm build (would require locally stubbing/patching
  `usdt`). With the other two handled, `usdt` is the only remaining error, so green is
  highly likely once it's gated — but it is **flagged, not claimed**.
- **Correction:** this revises the plan's earlier "wasm-clean / very likely green."
  Static dep-tree reading called `usdt` and `atomics` "opt-in features to gate off" —
  half right: `atomics` is feature-droppable, but **`usdt` is unconditional and needs
  upstream**, which only the build caught. Part 6's re-axing (wasm orthogonal to tier)
  stands architecturally; its wasm leg now carries a "pending small upstream patch"
  asterisk until the usdt gate lands.

### Update 2026-05-22 — the real wall is 64-bit, not usdt

Carried the build *past* all three blockers above (usdt gated out + a stubbed `ndt`
no-op module, getrandom configured, ArrayBuffer family dropped) and hit a deeper,
**structural** failure in `nova_vm`'s *own* code:

```text
error: evaluation panicked: assertion failed: size_of::<Value>() == size_of::<usize>()
error: attempt to shift left by `53`, which would overflow
```

Compile-time invariants: Nova's data-oriented `Value` is **register/`usize`-sized by
design**, and its integer math assumes **64-bit `usize`**. `wasm32-unknown-unknown` has
**32-bit `usize`**, so `Value` (which must carry an inline f64 / 32-bit handle + tag)
exceeds a 32-bit word and the assertions fail. Not a dep, not a feature flag — core to
Nova's heap design.

**Corrected verdict: `nova_vm` does NOT support wasm32, and it is NOT a small gate.**
Appendix B's "no wasm-incompat in Nova's own code / green highly likely" was wrong —
only the *full* build past usdt/atomics surfaced it. Making `Value`/arithmetic
32-bit-clean is upstream core work *against* Nova's 64-bit DOD premise, so unlikely soon.
`wasm64`/memory64 has 64-bit `usize` and would likely work, but browsers don't run
memory64 in production today. **Nova-in-the-browser is blocked.**

**Strategic resolution — the modular trait absorbs it.** Native target → **Nova**
(primary, as bet; this finding doesn't touch native). wasm target → **Boa** (pure Rust,
32-bit-clean, already runs in browsers) or quickjs. This is exactly what
`script-engine-api` + per-target backend selection is for — now load-bearing, not
theoretical. "Whole stack everywhere" holds in *capability* terms but **not
same-engine-everywhere**: the wasm scripted/fullweb cells run **Boa, not Nova**. The usdt
/ getrandom investigation edits (correct but moot given the 64-bit wall) were reverted;
`serval-embedder` carries only the EmbedderObject reflector patch.

**memory64 — the future escape hatch (assessed 2026-05-23; revised 2026-06-24, see update note).**

> **Update 2026-06-24 (wasm64-claims audit; supersedes the browser + toolchain facts in this section and the "Nova-in-the-browser is blocked" line above).** The structural reasoning below is correct (wasm64's 64-bit `usize` dissolves the wall), but two premises have flipped: (1) **Memory64 is default-on in Chrome/Edge 133 (2025-02-04) and Firefox 134 (2025-01-07)** — not "Chrome production, Firefox flagged"; only Safari/WebKit (desktop + iOS) still lacks it. (2) **wasm-bindgen now supports wasm64** (0.2.120, 2026-04-28, via an f64 pointer ABI; wasm-pack 0.15.0; getrandom 0.4.3 `wasm_js`), so "the glue isn't there… the killer for our use" no longer holds. Revised verdict: a Nova-on-wasm64 spike is **worth running now** on the two extension targets (thin client covers iOS). Honest remaining blockers, none permanent: Rust Tier-3 (nightly + `-Z build-std`, no panic=unwind); the `usdt` `compile_error!` on non-x86_64/ARM64, which must be target-gated and does **not** self-resolve on wasm64 (it was the sole remaining wasm32 probe failure); `ecmascript_atomics` feature-drop; ~10%-2x memory64 perf. The revisit trigger flips from "Safari ships memory64 AND wasm-bindgen supports wasm64" to just **Safari ships memory64**. Also: the precise wasm32 overflow sites are usize-typed — `MAX_UTF16_LENGTH = (1usize << 53) - 1` (`string/data.rs:66`) and `2usize.pow(53)` (`data_block.rs:322`), not the i64 `SmallInteger` math. See `2026-06-24_grand_audit.md` §6.

`wasm64-unknown-unknown` makes `usize` 64-bit, which *directly* dissolves this wall (the
`Value`-size assert and the 53-bit shift both pass). It is the path that could one day
collapse the native/wasm split back to Nova-everywhere. But shipping Nova-in-browser via
memory64 is blocked on a stack of immaturity, none of it ours to fix:

- **Browser coverage:** Chrome production; Firefox flagged; **Safari not implemented**
  (objection removed late 2025). Chrome-only ⇒ no cross-browser story; a PWA can't rely on it.
- **Rust target:** `wasm64` is tier-3, unstable, nightly + `-Z build-std`.
- **wasm-bindgen targets wasm32**; wasm64 interop support is limited — the JS↔wasm↔DOM
  glue we'd need basically isn't there. (The killer for our use.)
- **Perf:** browsers lose 32-bit-pointer optimizations on 64-bit memory — a slowdown
  stacked on Nova's interpreter.
- memory64 still wouldn't remove usdt / `ecmascript_atomics` / getrandom (wasm64 isn't
  x86_64/ARM64 either).

**Revisit trigger:** Safari ships memory64 **and** wasm-bindgen supports wasm64. Until
then Boa carries wasm. The `ScriptEngine` trait means adding a `wasm64 → Nova` backend
later is a swap, not a rearchitecture — so we don't bet on memory64's timeline.

Probe artifacts (throwaway, regenerable): `cargo new` lib depending on `nova_vm`, built
with `--target wasm32-unknown-unknown`; the decisive run set `nova_vm` and `usdt` to
`default-features = false` + both `getrandom` majors to `wasm_js` — leaving `usdt-impl`'s
arch `compile_error!` as the sole failure.

---

## Appendix C — Nova embedder/reflector API probe (2026-05-22)

Read the published `nova_vm 1.0.0` source directly (cargo registry) for the host-object +
rooting API the reflector design (Part 6, level 1) depends on. **Verdict: the rooting
discipline is solid and implemented, but the *reflector extension point itself is an
unimplemented stub* — embedding a DOM on Nova 1.0 is blocked on Nova-internal feature
work, not just immature-but-present APIs.** This is a bigger reprice than the wasm gate.

1. **`EmbedderObject` is a `todo!()` stub** (`builtins/embedder_object.rs`). Its own doc:
   "currently unimplemented but the intention will be that each embedder object is
   provided with a backing object reference while the embedder provides the data." It
   holds only an `Option<OrdinaryObject>`; every `InternalSlots` method is `todo!()`.
   **No way to attach native Rust data (a `NodeId`) yet.** This is exactly the reflector
   hook, and it isn't built.
2. **Native functions are non-capturing `fn` pointers** —
   `RegularFn = for<'gc> fn(&mut Agent, Value, ArgumentsList, GcScope) -> JsResult<Value>`.
   No closure capture, so the Appendix-A-Finding-1 capture footgun is moot in Nova's
   favor (nothing to capture), but **embedder state must reach native fns via the
   `Agent`/`HostHooks`**, not captures. `HostHooks` (passed at `GcAgent::new(.., &hooks)`)
   is the intended channel: a custom impl can hold the DOM-store handle. Cleaner than
   capturing, once mapped.
3. **The GC trace hook is crate-private.** Tracing is `HeapMarkAndSweep`
   (`mark_values`/`sweep_values`), `pub(crate)`, not a public `Trace`. **An embedder
   can't implement tracing for its own data that holds JS values** — the "reflector holds
   JS handlers, traced by Nova" half of the cycle story isn't embeddable yet either.
4. **Rooting works.** `Scoped<'scope, T>` (call-scope) and `Global<T>` (permanent, in
   `agent.heap.globals`) are implemented; `GcScope`/`NoGcScope` + `Bindable` enforce
   rooting at compile time as advertised. The DOM→reflector `Global` link is viable today.

### Consequence — two real ways forward

The clean reflector (native-data `EmbedderObject` + a public trace hook) is **upstream
feature work** — on Nova's stated roadmap but unbuilt; implementing a subsystem, not
patching one.

- **Interim `OrdinaryObject` reflector (buildable on 1.0 today):** expose each node as an
  ordinary JS object carrying its `NodeId` (hidden property, or an object-identity→NodeId
  side-table in `HostHooks`); JS handlers stored on it are normal properties Nova traces
  natively; the DOM holds a `Global` to it. Leaky (JS-visible NodeId) and slower
  (property lookups), but unblocks the whole binding layer now and swaps to
  `EmbedderObject` when upstream lands.
- **Co-develop `EmbedderObject` + public trace hook upstream** — the clean target; deeper
  commitment, contribution-friendly project.

The arena-isomorphism thesis is intact; the *mechanism* is partly unbuilt in Nova. This
elevates the "Nova primary" grind from "conformance + small wasm patch" to "also
co-develop Nova's embedder-object subsystem (or run the OrdinaryObject interim)."

**Decided 2026-05-22 — patch Nova (thin branch + `[patch.crates-io]`), not a heavy fork.**
The clean reflector *does* require a Nova source change (you can't add a struct field or
complete `todo!()` methods from outside — the only zero-source-change path is the leaky
OrdinaryObject interim), but the change is small and consumed cleanly:

- **The diff:** (a) add a `u32`/`u64` native-data slot to `EmbedderObjectHeapData` (enough
  to carry a `NodeId`) + implement its `todo!()` `InternalSlots` to **delegate to the
  existing backing `OrdinaryObject`**; (b) **no GC-internals surgery** — keep the
  reflector's JS state (event handlers) as properties on the backing object (Nova already
  traces it via `mark_values`/`sweep_values`) and the native data as a plain integer
  (nothing to trace); the crate-private trace hook only matters if native data holds
  `Value`s directly, which we avoid; (c) the **`usdt` target-gate** for wasm (Appendix B).
- **Mechanism:** the diff lives on a thin branch of `github.com/mark-ik/nova` (e.g.
  `serval-embedder`), consumed via `[patch.crates-io] nova_vm = { git = "...mark-ik/nova",
  branch = "serval-embedder" }` in the workspace root. Small, rebasable on upstream,
  **upstreamed as a PR** so the branch eventually disappears. Not a vendored in-tree fork.

The OrdinaryObject interim drops to an *optional throwaway smoke-test*, no longer the path.
**Sequenced behind Boa-first** so the `ScriptEngineLive` trait is validated against a
complete API *before* we also patch a Nova subsystem — otherwise a failure is ambiguous
between "trait wrong" and "our Nova patch wrong."
