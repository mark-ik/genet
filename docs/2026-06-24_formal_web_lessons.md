# Lessons from gterzian/formal-web

**Date:** 2026-06-24
**Status:** research / harvest. Applied into existing plans (see "Where applied" at the foot); not itself a plan.
**Source:** [github.com/gterzian/formal-web](https://github.com/gterzian/formal-web), a Rust web engine by Gregory Terzian (ex-Servo event-loop maintainer). Studied via a 6-agent fan-out reading the repo + Terzian's writing, mapped against serval's actual seams (file:line).
**Why it matters:** it is Boa-based (serval's own wasm JS backend), spec-faithful by construction, and its open problems are exactly serval's audit-named §6 async gaps (`2026-06-24_grand_audit.md`). Terzian fixed the same event-loop bugs in Servo that serval's message-passing constellation is structurally prone to. So this is a concrete playbook, not an analogy.

## What formal-web is

A deliberately spec-shaped Rust engine whose distinguishing trait is rigor, not features. Multiprocess (one OS process per similar-origin-window agent, WebKit/Apple-shaped). Three rigor mechanisms, all independent of that process architecture:

1. **TLA+ trace validation** (the real "formal" part; the Cirstea/Kuppe method, arXiv 2404.16075). The running engine emits an NDJSON event log via a `TLATracer`; `validate.rs` generates a `*TraceData.tla` constant module from the log; TLC replays that exact trace as the only allowed behavior against a refinement spec (`NavigationTrace.tla` does `Base == INSTANCE Navigation` and guards each action on `CurrentEvent`). The build fails if the implementation does something the spec forbids. Two specs per algorithm: a behavioral base (invariants like `TypeOK`) + a trace-refinement spec.
2. **Spec-step annotation discipline** (`AGENTS.md`): every step is `// Step N:` quoting verbatim spec text with matching numbering; doc comments are the spec anchor URL only (zero prose); `// Note:` is reserved for genuine code/spec discrepancies and globally budgeted under 10; named sub-algorithms become their own annotated functions; a three-layer Domain / WebIDL-infra / glue split; an end-of-task spec-mapping review audits the diff. Code becomes mechanically diffable against the spec.
3. **WPT-over-WebDriver runner** gated to `unexpected=0` via `include.ini` (skip-by-default, opt-in per file) + `meta/` expected-result files, plus local deterministic micro-tests that report via testharness.js OR a plain `window.__formalWebTestResult` object, mounted at `/__formal__/` to reuse upstream `/resources/testharness.js`.

## The case for taking the event loop seriously

Terzian used this toolkit to catch two real Servo event-loop bugs, and serval's "scenes travel as messages" constellation is especially prone to both:

- **Rendering-update atomicity.** The HTML "update the rendering" task is one atomic task with ordered steps; Servo had run those steps as separate scheduler messages. Decomposing an atomic task into messages is the natural (wrong) move in a message-passing kernel. (Fixing Servo's event loop, medium.com/@polyglot_factotum/fixing-servos-event-loop-490c0fd74f8d.)
- **Per-owner batching.** A global flag coalescing rendering work across documents stranded sibling documents when one closed. serval has multiple scenes/agents per loop, any tearable down: textbook setting. (Re-fixing Servo's event loop, medium.com/@polyglot_factotum/re-fixing-servos-event-loop-e00bdf267385.)

Both became **load-bearing invariants** in the actor-constellation plan (see Where applied).

## Transferable ideas (ranked), with serval seams

1. **BYOB byte-stream controller (the highest-value steal).** formal-web ships a complete `type:"bytes"` ReadableStream (`content/src/streams/readablebytestreamcontroller.rs` + `readablestreambyobreader.rs`: `byobRequest`, `respond(n)`, `respondWithNewView`, the `{min}` read option, element-size/alignment enforcement; `webidl/buffer_source.rs` rejects SharedArrayBuffer), exercised by `tests/formal/tests/byob-debug.html`. **Serval seam:** `components/script-runtime-api/fetch.rs:842` (`getReader` ignores `{mode:'byob'}`); the gap header is at `fetch.rs:452`. Same engine (Boa), same spec, ready-made conformance test. Medium effort, ships a real feature. This is the recommended first build.
2. **Task-source design that ports almost verbatim.** Channel-sender-per-source, per-source priority sorting, a priority watermark, self-converting messages, a wake-up sentinel (Programming Servo: the makings of a task queue). serval is also message-passing, so the data structures drop onto armillary channels. **Serval seam:** the WHATWG task boundaries are already named calls in `components/script-runtime-api/lib.rs` (`run_event_loop:266`, `run_timers:309`). Applied into the actor-constellation plan's task-source note.
3. **Tighten the microtask checkpoint (the FG-model fix).** serval pumps only around the whole timer batch; `lib.rs:264` literally flags "per-task checkpoints are a later refinement". The fine-grained ("FG") TLA+ model is what caught the atomicity bug. Move `pump_microtasks` inside the timer loop under the existing `Budget`/`pump` contract (`components/script-engine-api/lib.rs:184`). Low-medium effort.
4. **Spec-step annotation discipline.** Pure process, zero coupling, highest value-per-effort. Adopt across `components/script-*`; retrofit incrementally.
5. **Agent / agent-cluster as the concurrency boundary (the concept, not the process).** `Agent { id, can_block, event_loop_id }` / `AgentCluster` is spec-level data. Adopt agent-cluster as serval's SharedArrayBuffer/Atomics boundary; bind `EventLoopId` to an armillary actor (native) or a Web Worker (wasm) instead of an OS process. `can_block` (workers may block, windows may not) is directly the Web-Worker-as-agent plan. **Serval seam:** `repos/mere/crates/armillary/src/actor.rs:107`.
6. **Hand-written runtime WebIDL bindings registry (no codegen, no bootstrap-JS string).** A `WebIdlInterface` trait registers `OperationDef`/`AttributeDef`/`ConstantDef`; `register_interface_spec::<T>(ctx)` materializes the prototype and installs members as Boa `NativeFunction`s; platform objects are `#[derive(Trace, Finalize, JsData)]` Rust structs in `JsObject`s with `downcast_ref::<T>()` as the runtime type check; exotic objects use the public `JsProxyBuilder` (never Boa `pub(crate)` internals); async APIs keep promise resolvers in a side table keyed by `request_id`. **Serval seam:** replaces the ~900-line JS bootstrap string; the side table maps onto `new_host_promise`/`settle_host_promise` (`components/script-engine-api/lib.rs:201`); because algorithm bodies hold no `JsValue`, it supports the dual Boa/Nova goal. The public-API-only rule respects serval's no-fork-deps doctrine.
7. **TLA+ trace validation itself.** Architecture-agnostic (needs an event log + a model). serval's single-process model makes it *easier* than formal-web (one in-process channel, one counter clock, no cross-process monitor or channel-closure quiescence dance). The five WHATWG task boundaries are already named Rust calls to tap. Months-shaped CI investment, so do it for one protocol, not the engine.

## The solo-pace note

Terzian's recent writing argues LLMs collapse the expensive parts of formal methods (toy implementation, refinement-proof drudgery), leaving the human to author/validate a small high-level spec. That is the realistic adoption path at Mere's pace: spec the scary protocols (the event loop, port message races), let the model scaffold + trace-validate. The annotation discipline and the two bug-rules are free today; trace validation is the deferred, higher-cost capability.

## What does NOT transfer

formal-web's multiprocess-per-agent OS architecture is the opposite of serval's single-process in-process-actor constellation + wasm Web-Worker isolation. Do not copy:

- The `ipc/` OS-process bootstrap (`IpcOneShotServer`, Mach ports/fds, XPC backend, child-process handles). serval uses in-memory messages (native) or postMessage (wasm).
- The cross-process trace monitor (`TraceSender` clone-drop as global-quiescence detection, per-process `producer` tagging merged over IPC). serval needs one in-process mpsc + an explicit "trace complete" signal.
- `EventLoopId -> OS process` binding and multiprocess WindowProxy window-swapping.
- `wasmtime` for in-page wasm: serval's browser target bars a nested JIT and defers to the host `WebAssembly` object. Only the off-thread-compile + side-table-resolver + microtask-checkpoint *structure* transfers.

The agent concept transfers even though its process binding does not: copy the structs, the spec URLs, and the single-task-in-flight gate keyed on a completion ack; replace the spawn step.

## Where applied (2026-06-24)

- `repos/mere/.../2026-06-03_actor_constellation_plan.md` — the two event-loop bug-rules as load-bearing invariants; the task-source design note (idea 2); agent-cluster as the SAB/Atomics boundary.
- `2026-06-24_grand_audit.md` §6 — BYOB via formal-web's controller (idea 1) and the microtask-checkpoint tightening (idea 3) as the concrete §6 closers; pointer here.
- `2026-06-06_wasm_enablement_and_crate_rename_plan.md` — agent-cluster -> Web-Worker mapping + `can_block` in the async sub-thread.
- `2026-06-24_html_interface_table_plan.md` — the runtime WebIDL bindings registry (idea 6) as the reference shape for the table mechanism.
- `2026-06-24_wpt_harness_exactness_plan.md` — the WPT governance model (include.ini + meta/ + `unexpected=0` + dual-reporting micro-tests) and trace validation as an optional rigor capability (new H4).

## Spun off (new plans, this directory)

The two takeaways that are coherent workstreams without an existing home became scoped plans:

- `2026-06-24_byob_streams_plan.md` — idea 1, the BYOB byte-stream controller port (the recommended first build: bounded, ships a feature, reference impl + conformance test on the same engine/spec).
- `2026-06-24_event_loop_rigor_plan.md` — ideas 3/4/7 + the two bug-rules: per-task microtask checkpoint (E1), the atomicity invariants in the realization (E2), spec-step annotation discipline (E3), and optional TLA+ trace validation of the scheduler (E4, deferred).
