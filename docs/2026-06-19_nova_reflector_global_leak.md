# Nova reflector `Global` leak — problem + fix

**Date:** 2026-06-19. **Status:** FIXED (option B, the `NovaValue`
deferred-release wrapper). Root-cause repro committed `7fdb86bd579`; the fix
un-ignores it and the two pelt GC tests (`arg_reflector_dies_after_gc` + 7/7
nova engine tests green; 82/82 pelt scripted tests green across Boa and Nova).
**Affects:** `script-engine-nova` only (Boa is unaffected).

## Symptom

Two pelt scripted-tier GC tests fail **on Nova** but pass on Boa:
`pump_collects_orphans_on_nova` (a detached, dereferenced node is never reaped:
`8 -> 8`) and `gc_soak_bounds_memory_on_nova` (a `setInterval` churn peaks at
~12 000 live nodes instead of a handful — i.e. *nothing* is collected). Both are
`#[ignore]`d with a pointer to this doc.

## Root cause

`NovaCallCx`'s per-call minting methods — chiefly `arg(i)`, plus `make_string`,
`make_null`, `undefined`, `make_reflector`, `reflector_for` — return
`Global::new(agent, value)`. Nova's `Global` is a **heap-permanent** root: it
occupies an `agent.heap.globals` slot that must be released **explicitly** with
`Global::take(self, agent)`, and **`Global` has no `Drop`** (freeing needs the
`Agent`, which `Drop` cannot reach). So *dropping* a `Global` leaks its root
forever.

The native-call trampoline (`nova_trampoline`) carefully `take`s the original
argument handles and the result. But the generic DOM code obtains values via
`cx.arg(i)` (which mints a **fresh** `Global`) and simply drops them:

```rust
// e.g. setText / appendChild — runs for every node handoff
let node = cx.arg(0);              // Global::new(...) -> a heap-globals root
let id = cx.reflector_data(&node); // read the NodeId
// `node` dropped here -> the root leaks permanently
```

Because the DOM passes reflectors as arguments everywhere
(`parent.appendChild(child)`, `removeChild`, `insertBefore`, …), **every node's
reflector EmbedderObject is pinned forever**. The host pins the node while a
reflector exists (`reflect_pinned`) and only unpins when the reflector dies; the
reflector never dies, so the pin never retires and the node is never reaped.

Boa passes the same tests because its `Self::Value = JsValue` is `Rc`-based and
frees on `Drop`.

### Minimal repro

`script-engine-nova`'s `arg_reflector_dies_after_gc` (`#[ignore]`d): a reflector
passed as an argument to a native fn, then dropped by script, is still reported
*alive* after `force_gc` — pinpointing the leak to the argument-rooting path,
independent of the DOM.

## Why `CallCx::Value` is the wrong type

`ScriptEngine` binds `type CallCx<'a>: CallCx<Value = Self::Value>`
(`script-engine-api/lib.rs:91`): the **within-call** value type and the
**host-held** value type are forced to be identical. Host-held values
(`engine.eval`'s result, `set_global`) genuinely need a `Global` (they outlive
any call scope). Within-call values (`cx.arg`, intermediates) do **not** — they
live only for the native call. Forcing both to `Global` is what makes the
within-call path mint heap-permanent roots that then leak on drop.

## Upstream check (patches preferred)

`nova_vm` already ships the right primitive: **`Scoped<'scope, T>`**
(`engine/rootable/scoped.rs`) roots into `agent.stack_refs` for the duration of
the current call scope and is **released automatically** when the scope ends
(unlike `Global`, which is heap-permanent). It is documented as "for cheap
rooting of values that need to be used after calling into functions that may
trigger GC" — exactly the native-call case.

So there is **no `nova_vm` bug to patch**: `Global` having no `Drop` is by
design, and `Scoped` is nova's intended answer for call-scoped rooting. The fork
(`mark-ik/nova`, branch `genet-embedder`) is patchable, but a patch is not the
right layer — the misuse is in genet's adapter/API. (If a future fix wants a
nova-side convenience it would be additive, not a bug fix.)

## Fix options

### A. Use `Scoped` for the within-call path (nova-idiomatic)

Decouple `CallCx::Value` from `ScriptEngine::Value` so Nova's `CallCx::Value`
can be `Scoped<'a, Value<'static>>` (auto-released at the call scope) while
`ScriptEngine::Value` stays `Global<Value<'static>>` (host-held). Boa is
unchanged (its two value types coincide as `JsValue`).

- **Pro:** semantically correct — within-call values *are* call-scoped; release
  is automatic and immediate (no queue/drain machinery); uses the primitive nova
  intends.
- **Con:** an engine-neutral **API change** (drop the `Value = Self::Value`
  bound; the reflector round-trip host→JS→arg already goes through the VM, so the
  two types need not match), plus `'scope`-lifetime threading through
  `NovaCallCx` and the trampoline. Cross-cutting (touches `script-engine-api` and
  every backend's GAT), so more blast radius on GC-critical code.

### B. Deferred-release wrapper for Nova's `Self::Value` (Nova-local) — RECOMMENDED

Make Nova's `Self::Value` a small newtype `NovaValue { global: Option<Global>,
release: Rc<RefCell<Vec<Global>>> }` whose `Drop` moves its `Global` into a
shared **release queue** (it cannot free directly — no `Agent` in `Drop`). The
queue is drained — each `Global` `take`n with the agent — at two points that
hold the `Agent`: the **end of every native-call trampoline** (frees the call's
dropped temporaries immediately) and **`pump`/`collect_garbage`** (frees
host-held values dropped between calls). The result value returned to the VM is
unwrapped (not dropped through the queue), and the persistent canonical-reflector
cache keeps raw `Global`s (managed by `drain_dead_reflectors`, unchanged).

- **Pro:** **Nova-local** — no engine-neutral API change, Boa untouched; the
  blast radius is one crate. Release is bounded (each native call + each pump).
- **Con:** ~20 mint sites wrap their `Global`; a release queue + drain wiring;
  release of host-held values is deferred to the next drain (bounded, not
  immediate). Must `forget`/unwrap the result wrapper to avoid double-`take`.

**Recommendation: B.** The leak is a GC-correctness bug in one engine adapter;
fixing it without perturbing the engine-neutral API or the other two backends is
the lower-risk path. A is the cleaner long-term shape and can supersede B later
if the API is reworked for other reasons.

## Plan (option B)

1. `NovaValue` newtype + `Drop` (queue the `Global`); `Self::Value = NovaValue`
   for both `ScriptEngine` and `CallCx` on Nova.
2. A release queue (`Rc<RefCell<Vec<Global<Value<'static>>>>>`) on `NovaHostSlot`
   (reachable from cx methods, the trampoline, and `pump`/`collect` via the realm
   host slot).
3. Wrap every `Global::new` that becomes a `Self::Value`; leave the cache's
   `Global<WeakRef>` raw.
4. Drain the queue at the end of `nova_trampoline` and inside `pump` /
   `collect_garbage` (`take` each with the agent).
5. Un-ignore `arg_reflector_dies_after_gc`, `pump_collects_orphans_on_nova`,
   `gc_soak_bounds_memory_on_nova`; verify on Nova (and that Boa stays green).
