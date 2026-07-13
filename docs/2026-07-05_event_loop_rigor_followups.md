# Event-loop rigor follow-ups (spun out at archive)

**Date:** 2026-07-05. **Parent (archived):**
`archive/2026-06-24_event_loop_rigor_plan.md`. **Why:** the parent reached
all-phases-done (E1-E4) against its stated done-conditions and was archived.
This carries its optional/deferred residuals and gives the E3 annotation
convention a live home, since the convention governs future `script-*` code
and must not live only in an archived plan.

## The spec-step annotation convention (normative, carried from E3)

Applies to spec-mapped code across `components/script-*`. Landed on the
event-loop/task-source surface of `components/script-runtime-api/lib.rs`
(2026-06-30); apply to other spec-algorithm bodies as they are touched.

- Spec-mapped code uses `// Step N:` with short verbatim spec text for the
  step the next line realizes, numbering matching the spec.
- Genet embedding decisions use ordinary comments without `Step`; keep host
  policy separate from spec text.
- `// Note:` is reserved for real code/spec discrepancies, not implementation
  explanations; keep the global count small (parent budgeted under 10).
- Public Rust API docs may keep host-facing prose; private spec helper docs
  carry only the spec anchor URL.

## Open residuals

1. **Annotation generalization across `script-*`.** The parent annotated the
   HTML processing-model surface (checkpoint, task execution, task-source
   labels). Not yet under the convention: the DOM/fetch sub-algorithm bodies,
   and the event dispatch algorithm in
   `components/script-runtime-api/dom/bootstrap.js` (capture/target/bubble,
   currently prose DOM-spec cites around `bootstrap.js:432-535`). Judgment call
   (2026-07-05): the dispatch body is a deliberate simplification of DOM
   §dispatch (no shadow tree path construction or retargeting), so verbatim
   `// Step N:` quotes would mostly mint `// Note:` discrepancies against the
   budget; annotate it when dispatch is made spec-shaped (e.g. when Shadow DOM
   lands), and keep the prose cites until then. Elsewhere: retrofit
   incrementally as files are touched, per the convention above. No dedicated
   pass planned.
2. **Additional TLA+ protocol witnesses.** The trace harness, generator, CI
   wiring, and two witnesses (scheduler base, `postMessage` with a negative
   bad-trace check) are live. Candidate next protocols when one becomes scary
   enough to pay for: Navigation, MessagePort ordering, or the transitions
   plan's atomic rendering tick (clock -> rAF -> restyle -> layout -> paint as
   one task) once it exists. Do one protocol at a time; the harness cost is
   already sunk.
3. **Shared rAF callback queue** — *done 2026-07-05.* The `setTimeout(cb, 0)`
   stub is replaced by a real AnimationFrameProvider realization in
   `SHELL_GLOBALS_BOOTSTRAP` (`components/script-runtime-api/lib.rs`): callback
   map + identifier, spec-step annotated against
   `html.spec.whatwg.org/multipage/imagebitmap-and-animations.html`, snapshot
   pass semantics (mid-pass cancel honored, mid-pass registration lands next
   frame). `Runtime::run_animation_frame_callbacks(now_ms)` drains one callback
   per engine call with a microtask checkpoint between callbacks (a Promise
   reaction queued by callback N settles before callback N+1), traced as a
   scheduler boundary; `Runtime::has_animation_frame_callbacks()` lets hosts
   request frames only while script animates. Guards:
   `animation_frame_callbacks_on_boa` / `_on_nova`. The transitions plan's T2
   consumes this; its remaining work is tick ordering (rAF before the
   transition advance), which stays there.

   *Landing note:* adding the two guards shifted global document-tag
   allocation order in the `script-runtime-api` suite and deterministically
   surfaced a pre-existing failure in the E4 trace guards:
   `scheduler_trace_ndjson_on_boa`/`_on_nova` panic inside
   genet-scripted-dom's doc-tag fence ("NodeId from a different document
   (id tag 96, this doc 97)" / "NodeId refers to a live node") once enough
   runtimes have been created in the process. Repro:
   `cargo test -p script-runtime-api --lib` (any thread count); any 98-test
   subset passes, and the same four tests pass together in isolation, so the
   trigger is tag-counter magnitude, not any specific test. Hypothesis (not
   verified): tagged raw NodeIds, `(doc_tag << 48) | index`, cross the JS
   boundary as f64 in the dispatch/reflector paths and exceed exact-integer
   range once tags grow.

   *Resolved 2026-07-06 (hypothesis confirmed).* The transitions plan's event
   dispatch hit the same wall and pinned the mechanism: `dispatch_event`
   interpolated the raw node id into the eval string as a **bare number**
   (`__dispatchSynthetic({raw_node_id}, …)`), so a tagged id above 2^53 was
   parsed as an f64 and lost its low bits, which corrupted the doc-tag high bits
   and tripped the fence. Fix: pass the id as a **string** literal
   (`"{raw_node_id}"`) — the `__dispatch*` bridges already do `String(rawId)` —
   in both `dispatch_event` and the new `dispatch_transition_event`. The
   `scheduler_trace_ndjson` full-suite guards are green again. Any future
   raw-id-in-eval site must quote the id; the fetch-id sites (`__fetchPushChunk`
   etc.) are small counters and unaffected.

## Not carried

- Auto-firing the scripted-tier GC at the microtask checkpoint
  (`lib.rs:328` notes it as a one-line flip): a runtime concern, not an
  event-loop rigor residual; it stays with the GC work.
