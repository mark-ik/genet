# Event-model convergence plan (2026-06-01)

The audit's [#1 priority](./2026-05-29_serval_holistic_audit.md): serval has
**two** capture→target→bubble dispatchers, and they have already drifted. This
plan pins the divergence with evidence, defines the one propagation/cancellation
contract both must satisfy, and sequences the work to close the gap and keep it
closed.

## Why this matters

One DOM, one event model is the load-bearing synergy between Arc A (web
conformance: WPT event tests) and Arc B (serval-as-host: native handlers drive
the UI). The host plan states it directly ([host
plan](./2026-05-27_serval_as_host_xilem_serval_plan.md) §Gap 2): "One event
model, two entry points (native handlers and JS listeners), one propagation
algorithm." Today there are two algorithms, maintained independently, and a
behavior added to one silently fails to appear in the other.

## The two dispatchers (grounded, 2026-06-01)

They are necessarily *separate code* — different languages over different trees —
which is why they can't share a literal function and why a **shared contract +
conformance test** is the convergence mechanism, not a refactor-to-one-function.

| | JS dispatcher | Native dispatcher |
| --- | --- | --- |
| Where | `script-runtime-api/dom.rs` (`Node.prototype.dispatchEvent`, JS bootstrap) | `xilem-serval/src/runner.rs` (`dispatch_click`/`dispatch_key`, `phase_ordered_paths`) |
| Tree | JS DOM mirror, walks `parentNode` | xilem view-path chain, walks `dom.parent` + handler registry in `ServalCtx` |
| Effect | calls listener-array callbacks | routes `xilem_core` messages along `view_path`, collects bubbled `Action`s |
| Phase order | capture(root→target) · target(c then b) · bubble(target→root) | capture(chain reversed) · bubble(chain) — same order |

Both implement the **same propagation order correctly.** The drift is in the
*semantics that ride on top of it.*

## The drift, measured

Feature presence today (grep + read, not the stale audit):

| feature | JS side | native side |
| --- | --- | --- |
| capture / target / bubble order | yes | yes |
| `stopPropagation` | yes (`__stop`, dom.rs) | **no** |
| `stopImmediatePropagation` | **no** | **no** |
| `preventDefault` / `defaultPrevented` / `cancelable` | **yes** (lib.rs `Event` ctor + `preventDefault`; `dispatchEvent` returns `!__canceled`) | **no** |
| `once` listener option | yes | n/a (registry model) |
| passive listener option | **no** | **no** |
| `composedPath()` / `eventPhase` | **no** | **no** |
| `currentTarget` | partial | **no** |

(Verified 2026-06-01 against `lib.rs` `EVENT_TARGET_BOOTSTRAP` + `dom.rs`, not
the audit's prose. An earlier draft of this doc claimed `preventDefault` was
broken on the JS side — that was a mis-scoped grep; lib.rs implements it
correctly. Corrected here.)

Two conclusions:

1. **They have already forked on propagation control.** JS can halt propagation
   (`stopPropagation`); native cannot. A page relying on it behaves differently
   depending on which dispatcher fires — the silent divergence the audit feared.
2. **Cancellation exists in JS, is absent in native.** The JS path has a real
   `preventDefault`/`defaultPrevented`/`cancelable` (lib.rs) and
   `dispatchEvent` returns `false` when canceled. The native path has **no**
   cancellation concept at all — so the host cannot yet ask "did a handler
   cancel the default action?", which form controls and the pointer-drag/slider
   work need. That makes native cancellation the load-bearing gap for Arc B.

## The shared contract

One propagation/cancellation spec both dispatchers satisfy, drawn from the
[WHATWG DOM event dispatch
algorithm](https://dom.spec.whatwg.org/#concept-event-dispatch). The minimal
load-bearing subset (defer the rest explicitly):

- **Propagation order:** capture (root→target, exclusive of target) · target
  (listeners in registration order, capture-flag ignored at target per spec) ·
  bubble (target→root, only if `event.bubbles`). *Both already do this.*
- **`stopPropagation`:** no *later* node fires; listeners already queued on the
  *current* node still run.
- **`stopImmediatePropagation`:** no later listener fires, including remaining
  listeners on the current node.
- **`preventDefault`:** sets `defaultPrevented` iff `cancelable`; `dispatchEvent`
  returns `false` iff `defaultPrevented`. The host reads this to decide whether
  to run the default action (form activation, drag start, caret move).
- **`once`:** listener removed after first invocation (JS has it; native registry
  is single-listener so n/a).
- **`currentTarget`:** the node whose listeners are firing; reset after.

**Deferred (named, not silently dropped):** `composedPath` / shadow trees
(serval has no shadow DOM yet), passive listeners + scroll-blocking semantics,
`eventPhase` as an observable constant, retargeting. None blocks the two arcs
today.

## The convergence mechanism: a shared scenario table, asserted on both sides

The thing that *keeps* them converged. The two dispatchers live in **separate
dependency islands** — `xilem-serval` (native) does not depend on
`script-runtime-api` (JS, which pulls Boa/Nova), and coupling them just to share
a test would be wrong. So the cross-path conformance test is **one scenario
table, asserted independently in each crate, with explicit cross-references** so
that editing one side without the other reads as an obvious inconsistency.

The shared scenario table (each row asserted on both sides):

| scenario | expected firing order | `defaultPrevented` |
| --- | --- | --- |
| plain bubble (child + parent listen) | child, parent | false |
| non-bubbling event | child only | false |
| `stopPropagation` at child | child only (parent skipped) | false |
| `stopImmediatePropagation` at a node with 2 listeners | first listener only; later nodes skipped | false |
| `preventDefault` | handler fires | **true** |

- **JS column:** `script-runtime-api`'s `dom_node_events_work<E>` (run on Boa +
  Nova), asserting the `console` firing log.
- **Native column:** `xilem-serval`'s `stop_propagation_halts_the_bubble_walk`
  and `prevent_default_is_visible_to_the_caller`, asserting the `Log` firing
  vector and the shared `Propagation` cell.

Each test's doc comment names its twin and points here. Drift in one without the
other is a visible inconsistency a reviewer (or a grep for the doc path) catches.

## Sequencing

1. **Close the JS-side gap** — **done (2026-06-01).** `stopImmediatePropagation`
   added to `dom.rs` (`Event.prototype` + the `fire` loop checks `__stopImmediate`
   per-listener; it implies `stopPropagation`). `preventDefault`/`cancelable`/
   `defaultPrevented` already worked (lib.rs `EVENT_TARGET_BOOTSTRAP`). Verified
   on Boa + Nova (`dom_node_events_work`).
2. **Add cancellation to the native dispatcher** — **done (2026-06-01).** A
   shared `Propagation` handle (`Rc<Cell<…>>`, `propagation.rs`) embedded in
   `PointerClick` + `KeyEvent`; clones share one cell, so a handler calling
   `ev.stop_propagation()` / `ev.prevent_default()` (methods take `&self`) is
   seen by the dispatch loop (which `break`s on `stopped()` between routed paths)
   and by the host (which reads `default_prevented()` via a pre-dispatch clone of
   the handle — the seam the slider/drag work needs). Verified in `xilem-serval`.
   Note: this dispatcher registers ≤1 listener per node per type, so
   `stop_immediate_propagation` ≈ `stop_propagation` natively; both flags exist
   to match the JS contract exactly.
3. **Cross-path conformance** — **done (2026-06-01).** Realized as the shared
   scenario table above, asserted in each crate (separate dep islands; see the
   mechanism section), with each test's doc comment naming its twin + this doc.
4. **Record the deferred set** in this doc as it changes; revisit shadow
   DOM / passive when a consumer needs them.

## dom/events conformance push (2026-06-01)

With the contract converged, the JS side got the rest of the event surface the
WPT `dom/events` suite exercises. The push found that **`document.createEvent`
was a universal blocker** — absent, so every legacy test (which builds events via
`createEvent(iface)` + `initEvent`) threw at construction and *every* subtest
failed regardless of the feature it tested. Fixing it doubled the suite alone.

Landed (all in `dom.rs` + `lib.rs`, both engines where applicable):

- **`createEvent(iface)` + `initEvent(type, bubbles, cancelable)`** with the DOM
  initialized/dispatch flags: an uninitialized or mid-dispatch event throws
  `InvalidStateError`; a recognized-but-unmodeled interface yields a base `Event`
  (no per-interface subclasses yet); an unknown interface is `NotSupportedError`.
- **`addEventListener` options object** `{capture, once, passive}` (plus the
  legacy bool `capture`); listeners stored as `{cb, once}` records, deduped by
  `(type, callback, capture)`; a `once` listener is removed before it fires.
  `passive` is parsed/accepted but has no scroll-default to gate yet.
- **`composedPath()`** (the recorded target→root path; no shadow boundary yet)
  and **`eventPhase`** (NONE/CAPTURING/AT_TARGET/BUBBLING, live during dispatch +
  `Event.*` constants).
- **Legacy aliases**: `event.returnValue` (↔ preventDefault), `event.cancelBubble`
  (↔ stopPropagation), `event.srcElement` (↔ target).

Result: **dom/events 66 → 142 subtests, 3 → 13 all-pass files** (Boa). The shared
conformance scenario table grew `once` + a `createEvent`/`initEvent` round-trip,
still asserted on Boa + Nova.

Still open (named, not silently dropped): `window`/`document` as targets in the
propagation path + table-internal trees (`Event-dispatch-bubble-canceled`);
per-interface event subclasses (MouseEvent etc. — `createEvent` returns a base
Event); shadow DOM / `composedPath` retargeting; passive scroll-blocking;
`currentTarget` on the native side.

Steps 1 and 3 are pure-mine (dom.rs + a test crate). Step 2 touches the runner —
coordinate so it doesn't land on top of the agent's in-flight pointer work.

## Receipts to update as this lands

- WPT: `serval-wpt testharness dom/events/<subset>` pass count before/after
  step 1 (the event-dispatch conformance tests).
- A green cross-path conformance test (step 3) — the standing anti-drift guard.
