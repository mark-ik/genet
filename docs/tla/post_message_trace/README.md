# postMessage trace validation

This is the first protocol-shaped E4 witness for
`docs/archive/2026-06-24_event_loop_rigor_plan.md`.

`components/script-runtime-api` emits `post_message` enqueue/deliver marks through
`Runtime::scheduler_trace_ndjson()`. Generate the TLA+ data module with:

```sh
python3 components/script-runtime-api/tools/scheduler_trace_to_tla.py \
  components/script-runtime-api/tests/fixtures/post_message_trace.ndjson \
  docs/tla/post_message_trace/PostMessageTraceData.tla \
  --module-name PostMessageTraceData
```

`PostMessageTraceData.tla` is checked in from the good fixture trace so CI can
detect fixture/data drift before it runs TLC. CI also generates a bad
same-turn-delivery fixture from
`components/script-runtime-api/tests/fixtures/post_message_trace_bad_sync.ndjson`
and expects TLC to reject it.

Run TLC from this directory with:

```sh
java -cp tla2tools.jar tlc2.TLC -config PostMessageTrace.cfg PostMessageTrace.tla
```
