# BYOB byte-stream plan (type:"bytes" ReadableStream)

**Date:** 2026-06-24
**Status:** plan. Spun out of the gterzian/formal-web harvest (`2026-06-24_formal_web_lessons.md`, idea 1) and the grand audit §6 BYOB gap. The highest-value, bounded steal: it closes an audit-named gap against the *same* JS engine (Boa) and *same* spec, with a ready-made reference implementation and conformance test.
**Thesis:** genet's `ReadableStream` is buffered and `getReader` ignores `{mode:'byob'}` (`components/script-runtime-api/fetch.rs:842`; the gap is flagged at `fetch.rs:452`). formal-web ships a complete `type:"bytes"` controller; port it onto genet's stream surface rather than re-derive the Streams spec's pull-into machinery.

## Reference (do not re-derive)

formal-web's byte-stream files, against Boa:
- `content/src/streams/readablebytestreamcontroller.rs` — the controller: `byobRequest`, `enqueue`, `respond(n)`, `respondWithNewView(view)`, the pull-into descriptor queue, auto-allocate, close/error.
- `content/src/streams/readablestreambyobreader.rs` — the BYOB reader: `read(view, {min})`, element-size/alignment enforcement, partial-fill/close/cancel.
- `content/src/webidl/buffer_source.rs` — `get_a_copy_of_the_buffer_source`, ArrayBufferView descriptor (element size, alignment), SharedArrayBuffer rejection.
- `tests/formal/tests/byob-debug.html` — the conformance micro-test (min-zero rejects, read-min-then-read, close-before-fill, respondWithNewView, cancel-partial-fill, Uint16-against-3-byte-buffer TypeError).

## Phases (done-conditions, not dates)

### B1 — Failing micro-test first

Add a genet micro-test mirroring `byob-debug.html` (the six cases above), reporting via `window.__formalWebTestResult` (per the WPT-harness plan's H4 governance), landed RED against the current buffered reader at `fetch.rs:842`.
- **Done when** the micro-test exists, runs in the harness, and fails for the right reason (no BYOB reader), pinning the target behavior before any port.

### B2 — Port the byte controller + BYOB reader

Bring over the pull-into descriptor queue, the `ArrayBufferViewDescriptor` (element size + alignment), `respond(n)` / `respondWithNewView(view)`, the `{min}` read option, and SharedArrayBuffer rejection. Re-verify every Boa API against genet's Boa pin (the fork may have skewed since formal-web's pin; this is the main porting friction).
- **Done when** the B1 micro-test passes, including the alignment/element-size TypeErrors and the close/cancel edge cases.

### B3 — Wire `getReader({mode:'byob'})`

At `fetch.rs:842`, route `{mode:'byob'}` to the new BYOB reader; the default (buffered) reader path stays for `getReader()`. A byte controller also auto-allocates buffers for default reads when configured, so confirm the default path still behaves.
- **Done when** both reader modes work off one stream and the buffered path is unregressed.

### B4 — Score it on WPT

Enable the `streams/readable-byte-streams/` (and related) WPT directory under the harness governance (H4: opt-in `include.ini` + `meta/` expectations), and publish the delta.
- **Done when** the byte-stream WPT slice runs green-by-default with a tracked aggregate.

## Sequencing

B1 -> B2 -> B3 -> B4. B1 first is the formal-web discipline (pin the target with a failing test). B4 depends on the WPT-harness plan's H1/H2/H4 being far enough along to enable a directory cleanly; if not, B1's micro-test still locks the behavior in the interim.

## Non-goals

- A full Streams rewrite; this adds the byte/BYOB lane to the existing stream surface.
- Transferable streams / `tee` BYOB beyond the spec minimum.
- True-async stream producers (tracked as a granularity item in the event-loop rigor plan / grand audit §6), distinct from BYOB readers.

## Findings

- 2026-06-24 (grand audit + formal-web harvest): genet's ReadableStream is buffered with no BYOB; `getReader` ignores `{mode:'byob'}` (`fetch.rs:842`). formal-web's controller is a direct reference on the same engine + spec. Main risk is Boa API skew vs genet's pin, surfaced in B2.

## Progress

- 2026-06-24 — Plan created from the formal-web harvest. No code yet. B1 (the failing micro-test) is the entry point.
