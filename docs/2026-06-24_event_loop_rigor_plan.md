# Event-loop rigor plan (granularity + spec fidelity + optional trace validation)

**Date:** 2026-06-24
**Status:** plan. Spun out of the gterzian/formal-web harvest (`2026-06-24_formal_web_lessons.md`, ideas 3/4/7 + the two bug-rules) and the grand audit §6 ("the gaps are granularity, not architecture").
**Thesis:** serval models the WHATWG event loop on engine-neutral primitives (microtask checkpoint, timer task source over a virtual clock, capture/target/bubble dispatch), tested on both Boa and Nova. The shape is right; what is missing is **granularity** (coarse microtask checkpoints, atomic tasks at risk of decomposition) and **rigor** (no mechanical spec-diff, no model check). This plan tightens the granularity, encodes the two bug-rules Terzian fixed in Servo, adopts the spec-annotation discipline, and offers model-checked trace validation as a deferred capability. Cheapest correctness wins first; the heavy rigor last.

## Phases (done-conditions, not dates)

### E1 — Tighten the microtask checkpoint (per-task, not per-batch)

serval pumps microtasks only around the whole timer batch; `components/script-runtime-api/lib.rs:264` literally flags "per-task checkpoints are a later refinement". The fine-grained ("FG") model is what caught the real Servo bug. Move `pump_microtasks` inside the timer loop, draining after each task, under the existing `Budget`/`pump` contract (`components/script-engine-api/lib.rs:184`).
- **Done when** a microtask queued by task N runs before task N+1 (not after the batch), verified by a deterministic micro-test (per the WPT-harness H4 governance), on both Boa and Nova.

### E2 — Encode the two atomicity invariants in the realization

The constellation realizes spec tasks as messages, which is precisely where Terzian's two event-loop bugs live (now load-bearing invariants in `repos/mere/.../2026-06-03_actor_constellation_plan.md`):
- **Atomic task = one message.** The "update the rendering" task and its ordered sub-steps must not be split across separate scheduler messages. Audit serval's render/scene scheduling and meerkat's content-actor message decomposition for any sub-step fan-out.
- **Per-owner batching.** Any coalescing of work across independently-lifetimed owners (documents, tiles, scenes, agents) is scoped per owner; no global batching flag that strands siblings on teardown.
- **Done when** no atomic spec task is realized as multiple messages, coalescing state is per-owner, and a teardown-during-batch test does not strand siblings.

### E3 — Spec-step annotation discipline (start at the event loop)

Adopt formal-web's `AGENTS.md` discipline, beginning with the event-loop + task-source algorithms in `script-runtime-api`, then generalize across `components/script-*`: `// Step N:` quoting verbatim spec text with matching numbering; doc comments are the spec anchor URL only; `// Note:` reserved for genuine code/spec discrepancies, globally budgeted under 10; named sub-algorithms become their own annotated functions.
- **Done when** the event-loop modules are mechanically diffable against the HTML processing-model spec text, and the convention is recorded for the rest of `script-*`.

### E4 — TLA+ trace validation of the scheduler (deferred, optional rigor)

Architecture-agnostic and *easier* for serval than formal-web (single-process: one in-process channel + one counter clock, no cross-process monitor or channel-closure quiescence dance). Tap the five named task boundaries (`lib.rs:266` `run_event_loop`, `:309` `run_timers`, `dispatch_event`, `eval`, `pump_microtasks`) to emit an NDJSON event log; write one base + trace TLA+ spec pair for one protocol (the event loop, or Navigation / MessagePort, mirroring formal-web); generate the `*TraceData.tla` constant module from the log and run TLC in CI (the Cirstea/Kuppe method, arXiv 2404.16075). Use the FG model (per-task `running` flags + the lockstep-counter invariant) so it can catch the E2-class bugs automatically.
- **Done when** a recorded run is model-checked to refine the spec for one protocol, and a deliberately-wrong scheduling change (e.g. splitting the rendering task) fails the check in CI.

## Sequencing

E1 -> E2 -> E3 -> E4. E1 is a small correctness fix that ships immediately. E2 hardens the realization against the known bug classes (and is mostly verification + targeted fixes). E3 is free discipline that makes E4 tractable. E4 is the months-shaped capability; gate it on the LLM-assisted-spec workflow being worth standing up (Terzian's argument that LLMs collapse the expensive parts is the affordability case at solo pace). Do E4 for one protocol, not the engine.

## Findings

- 2026-06-24 (grand audit + formal-web harvest): serval's event-loop gaps are granularity, not architecture; the coarse microtask checkpoint is flagged in-code at `lib.rs:264`. Terzian caught two real Servo event-loop bugs (rendering-task atomicity; cross-document batching) with the FG model + trace validation; serval's message-passing constellation is structurally prone to both. Single-process makes trace capture easier than formal-web's multi-process monitor.

## Progress

- 2026-06-24 — Plan created from the formal-web harvest. No code yet. E1 (per-task microtask checkpoint) is the entry point and the cheapest correctness win.
