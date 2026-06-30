# Scheduler trace validation

This is the E4 witness for `docs/2026-06-24_event_loop_rigor_plan.md`.

`components/script-runtime-api` emits scheduler trace NDJSON through
`Runtime::scheduler_trace_ndjson()`. Generate the TLA+ data module with:

```sh
python3 components/script-runtime-api/tools/scheduler_trace_to_tla.py \
  components/script-runtime-api/tests/fixtures/scheduler_trace.ndjson \
  docs/tla/scheduler_trace/SchedulerTraceData.tla
```

`SchedulerTraceData.tla` is checked in from the fixture trace so the spec is
immediately runnable and CI can detect fixture/data drift.

Then run TLC from this directory:

```sh
java -cp tla2tools.jar tlc2.TLC -config SchedulerTrace.cfg SchedulerTrace.tla
```

CI runs the same path through `support/ci/check_scheduler_trace_tla.py`.
