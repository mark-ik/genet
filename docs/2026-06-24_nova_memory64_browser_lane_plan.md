# Nova-on-Memory64 experimental browser lane

**Status (2026-06-24): IMPLEMENTED through the dual-artifact build and private
worker protocol; browser-matrix CI and measurements remain the promotion gate.**

This document is the authority for Serval's in-browser script-engine selection.
It supersedes every earlier statement that Nova is structurally native-only, that
production browsers do not support Memory64, or that wasm64 glue is unavailable.

Memory64 is a Phase 4 WebAssembly feature and is available in Chrome/Edge 133+ and
Firefox 134+. Safari/WebKit remains the unsupported platform. `wasm-bindgen`
supports wasm64 and Serval locks 0.2.126. Rust's `wasm64-unknown-unknown` target is
still Tier 3, so the Nova build stays on a pinned nightly with `rust-src` and
`-Z build-std=std,panic_abort`.

References: [Memory64 status](https://github.com/WebAssembly/memory64),
[browser matrix](https://github.com/mdn/browser-compat-data/blob/main/webassembly/memory64.json),
[wasm-bindgen wasm64 support](https://github.com/wasm-bindgen/wasm-bindgen/pull/5004),
[Rust target status](https://doc.rust-lang.org/nightly/rustc/platform-support/wasm64-unknown-unknown.html).

## Landed architecture

- `script-engine-nova` and `serval-scripted` gate Nova on
  `target_pointer_width = "64"`, admitting native64 and wasm64 while rejecting
  wasm32.
- The Serval Nova fork gates USDT off `target_family = "wasm"`, supplies the same
  no-op registration/probe surface, enables both `getrandom` JavaScript backends,
  and carries a single-worker `ecmascript_atomics` backend for ordinary
  ArrayBuffer/TypedArray storage.
- The Nova wasm artifact excludes `shared-array-buffer`, `atomics`, and Temporal.
  The worker also masks `SharedArrayBuffer` and `Atomics` at bootstrap so the Boa
  fallback has the identical baseline capability profile. No threads, shared Wasm
  memory, COOP, or COEP are required.
- `ScriptedDocument<E>` is public from `serval-scripted`; Pelt re-exports and
  consumes it. Its runtime/DOM/extraction core builds with
  `default-features = false`; layout and scene production are a separate `render`
  extension and do not enter either worker artifact.
- `serval-scripted-worker` is backend-exclusive. `engine-nova` and `engine-boa`
  cannot co-link. `WorkerSession` owns the engine, `Runtime<E>`, `ScriptedDom`,
  microtask queue, reflector lifecycle, and GC cadence.
- `worker-bootstrap.mjs` exposes one private protocol: `initialize`, `evaluate`,
  `evaluate-module`, `dispatch-event`, `snapshot`, `collect-garbage`, and
  `shutdown`. Node ids cross as decimal strings, avoiding wasm64 pointer truncation
  through JavaScript Number.
- `loader.mjs` calls `WebAssembly.validate()` on the checked-in minimal Memory64
  module `(module (memory i64 1))`. A successful validation attempts Nova; worker,
  glue, or instantiation failure terminates that worker and retries Boa with the
  original initialization input. The ready handshake reports engine, pointer
  width, shared-memory profile, and the initial neutral DOM snapshot.
- `support/build-scripted-workers.ps1` pins nightly `nightly-2026-06-22`, requires
  `wasm-bindgen` CLI 0.2.126, and emits:
  - `serval-scripted-nova-wasm64`
  - `serval-scripted-boa-wasm32`

This lane stays below `ScriptEngine` / `script-runtime-api`. It does not change or
merge with the higher-level `DocumentScript` component contract.

## Build and test commands

```powershell
./support/build-scripted-workers.ps1
node --test components/serval-scripted-worker/loader.test.mjs
cargo test -p serval-scripted-worker
cargo test -p serval-scripted-worker --no-default-features --features engine-nova
```

The raw Rust artifacts are independently buildable with:

```powershell
$env:RUSTFLAGS='--cfg getrandom_backend="wasm_js"'
cargo +nightly-2026-06-22 build -p serval-scripted-worker `
  --no-default-features --features engine-nova `
  --target wasm64-unknown-unknown -Z build-std=std,panic_abort

cargo +1.95.0 build -p serval-scripted-worker `
  --no-default-features --features engine-boa `
  --target wasm32-unknown-unknown
```

The checked-in tests cover capability validation, forced Nova failure with
lossless Boa retry, pointer-width/engine diagnostics, DOM mutation, ArrayBuffer and
TypedArray, SharedArrayBuffer/Atomics absence, and microtask settlement. The moved
`serval-scripted` suite remains the shared backend-neutral fixture set for event
dispatch, reflector retirement, GC/node churn, modules, and neutral snapshots.

## Promotion gate

The lane remains experimental until browser CI runs the scripted-DOM suite on
Nova/wasm64 in Chrome and Firefox and Boa/wasm32 in WebKit, confirms matching shared
snapshots, exercises host termination/restart against a runaway worker, and records
bundle size, startup latency, and memory. Safari support is not a stop condition;
Boa is the compatibility artifact. Incremental layout and serialized-scene transport
are the next milestone, not part of this one. A threaded SharedArrayBuffer/Atomics
profile is a separate project.
