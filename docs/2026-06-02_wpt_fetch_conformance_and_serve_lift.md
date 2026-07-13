# WPT fetch/ conformance: JS surface, netfetcher binding, serve lift

Status: **2026-06-02.** Stood up the JS Fetch API surface, wired `fetch()` to a
host seam, backed that seam with netfetcher in `genet-wpt`, and taught the
runner to wrap `.any.js` tests. The network-free `fetch/api/` subset runs and
scores on both engines (was 0 before). Both gates are now cleared: the hosts file
is set up (Gate A) and `genet-wpt` has a **server mode** (Gate B) that drives the
network-dependent `fetch/` corpus against a live `wpt serve`. Network fetch tests
that were previously impossible now pass against the real server on both engines
(e.g. `fetch/api/basic/http-response-code` 1/1, `mode-same-origin` 6/8).

Update **2026-06-03.** Inverted the host seam from a synchronous pull to a
**deferred push** (see "The deferred fetch seam" below), so `fetch()` runs
asynchronously: mid-flight `AbortController.abort()` now cancels an in-flight
request, and response bodies can stream incrementally. This is the seam Mere's
actor model wants (fetch replies delivered as messages, not `block_on`). Sync
hosts still settle in-tick, so nothing in the prior surface regressed.

## What landed

### 1. The `fetch()` host seam (`components/script-runtime-api/fetch.rs`)

The runtime crate links no network stack. `fetch()` crosses a sync trait object:

```rust
pub struct FetchRequest  { method, url, headers, body }
pub struct FetchOutcome  { network_error, status, status_text,
                           response_type, url, headers, body }
pub trait FetchHandler { fn fetch(&self, request: FetchRequest) -> FetchOutcome; }
```

`HostState` carries `fetch: Option<Box<dyn FetchHandler>>`, set via
`Runtime::set_fetch_handler`. The native `__fetch` sink reads four string args
(method, url, newline-delimited headers, body), calls the handler, and encodes the
outcome as hand-rolled JSON (no JSON dep). With no handler installed it returns a
network error, which is the spec-correct default.

### 2. The JS Fetch API surface (`FETCH_BOOTSTRAP`)

Bootstrapped in JS over the single `__fetch` sink:

- **Headers**: RFC 7230 name validation (token regex), OWS-trimmed values,
  `append`/`set`/`get`/`has`/`delete`/`getSetCookie`, sorted iteration
  (`entries`/`keys`/`values`/`Symbol.iterator`/`forEach`).
- **Request**: full object, `GET`/`HEAD`+body throws, `clone`.
- **Response**: `new Response(body, init)`, status-range check, `error`/
  `redirect`/`json` statics, `clone`.
- **Body mixin**: `text`/`json`/`arrayBuffer`, single-use via a consumed flag.
- **`fetch()`**: takes a Request, adds a default `Accept`, joins headers with
  `"\n"`, builds a Response from the outcome.

The separator is a literal newline on both sides (JS `join("\n")`, Rust
`split('\n')`).

### 3. netfetcher-backed handler (`ports/genet-wpt`, `netfetch` feature)

`NetFetchHandler` holds a current-thread tokio runtime and implements
`FetchHandler` by `block_on`-ing `netfetcher::fetch`, mapping Method, response
type, final URL (`url_list.last()`), and body bytes. Gated behind the optional
`netfetch` feature so the default `genet-wpt` build stays free of the async net
stack (confirmed unaffected).

### 4. `.any.js` wrapping in the runner (`src/main.rs`)

`is_any_js` matches `.any.js`/`.window.js`/`.worker.js`; `synthesize_any_js`
parses `// META: script=` and `global=` directives and builds a wrapper HTML
(testharness.js + meta scripts + the file). Worker-only globals are skipped. This
is what makes the `fetch/api/` `.any.js` corpus runnable at all.

## Network-free numbers (both engines, release `genet-wpt`)

| subset | boa subtests | nova subtests |
|---|---|---|
| fetch/api/headers | 181/205 | 181/267 |
| fetch/api/request | 318/545 | 318/505 |
| fetch/api/response | 202/290 | 202/290 |

All three were **0** before this work (the files would not even parse and run).
These are the headers/request/response object-semantics tests that need no live
server. Boa and nova agree on pass counts everywhere; the denominators differ only
in a generated-subtest-count tail.

A 2026-06-04 conformance pass on the object-semantics surface (no new network
needed) lifted these from 86 / 257 / 196 via, in order: header value
normalization + init validation + live prototyped iterators; the `self.GLOBAL`
stub in the `.any.js` wrapper; WHATWG **header guards** (forbidden request-header
+ no-CORS safelist on requests, forbidden response-header on responses) — the
single biggest win (`headers-forbidden-override` 18 -> 90/90); **Request
constructor validation** (method / init-enum / URL / no-`new`); and multipart
`formData()` parsing. The real-time timer gate (below) is what makes
timer-scheduled abort fire.

Two passes drove the request / response jumps (request 168 -> 246, response
46 -> 160, and more files executing instead of erroring):

- **Fetch globals.** `URLSearchParams`, `Blob` / `File`, `FormData`,
  `TextEncoder` / `TextDecoder` were missing, so a top-level reference to any
  aborted the whole `.any.js` file. They now exist, with WHATWG body extraction
  wiring them as request / response bodies (correct `Content-Type` per type), plus
  `blob()` / `formData()` accessors.
- **ReadableStream.** A buffered `ReadableStream` (+ default reader) and
  `response.body` / `request.body` as streams, with `Response`-from-stream body
  extraction. This took response from 96 to 160 (`response-consume-stream` 14/15,
  the `response-stream-disturbed-*` family 8/12). A later pass made the
  stream-backed body **lazy** (a `Response`/`Request` built from a stream keeps the
  stream, consumed on demand) with correct lock/disturb semantics: a locked or
  disturbed input stream is rejected, consuming locks the body so
  `body.getReader()` then throws, reading the original stream disturbs the
  response, and a non-`Uint8Array` chunk fails consumption. That took response to
  182/290 — `response-from-stream`, `response-stream-bad-chunk`, and
  `response-stream-disturbed-6` now pass in full, and `disturbed-5` reaches 8/12.
- **`WritableStream` / `TransformStream` + `pipeTo` / `pipeThrough`.** A buffered
  `WritableStream` (+ default writer) and a `TransformStream` (identity /
  `transformer.transform`); `ReadableStream.pipeTo` / `pipeThrough` lock and
  disturb the source synchronously (so `bodyUsed` flips immediately, per spec) then
  pump best-effort. `response-stream-disturbed-by-pipe` now passes (2/2); response
  reaches 184/290. Byte (BYOB) readers and genuinely async producers stay deferred.
- **AbortController / AbortSignal.** `AbortSignal` is an `EventTarget` (abort
  event + `onabort`), with `throwIfAborted` and the `abort` / `timeout` / `any`
  statics; `fetch(url, {signal})` rejects a pre-aborted signal with its reason.
  Took `fetch/api/abort` from erroring (no `AbortController`) to 37/88 (general
  25/53, request 12/18); the rest there need live-network mid-flight abort.
- **`URL`.** The WHATWG `URL` object + `searchParams`, backed by the Rust `url`
  crate via two natives (`__url_parse`, `__url_with`) rather than a JS reimpl, so
  parsing and component setters are spec-correct. Foundational and used widely;
  within `fetch/` its remaining consumers (`*/url-parsing.html`, abort) are gated
  on iframes and mid-flight abort, so the network-free subtest delta is small, but
  it removes a class of `new URL` failures and is exercised by the binding tests.
- **Binary body channel.** The internal body is now raw bytes (`__bytes`), and it
  crosses the `__fetch` sink as a lossless "binary string" (each char code 0-255 =
  one byte; the Rust side maps chars <-> bytes). So `Blob` / `ArrayBuffer` /
  typed-array request bodies and binary responses are byte-exact end to end
  (proven by a 0..256 round-trip binding test), where before they degraded through
  a UTF-8 round-trip. Request +7 (request-consume binary subtests). Body accessors
  compute synchronously and resolve with the final value, so `text()` stays immune
  to a poisoned `Object.prototype.then` (the broken-then tests).

## The `wpt serve` lift: blocked, two gates

The full WPT server tooling is vendored (`tests/wpt/tests/` has the `wpt` CLI,
`tools/serve/serve.py`, `tools/certs/`, the `fetch/` Python handlers). Python
3.14.2 is present; `wpt serve --exit-after-start` bootstraps its venv and deps
cleanly. It then fails:

```text
CRITICAL - start_http_server: getaddrinfo failed
Please ensure all the necessary WPT subdomains are mapped to a loopback device.
```

**Gate A, hosts file. CLEARED 2026-06-02.** `wpt serve` resolves
`web-platform.test` and its subdomains for its own readiness probe. A
localhost-only `--config` override does not sidestep this (`subdomains` is not even
a valid config-override key, which raises `KeyError`). The fix is the documented
one-time admin step, run once on this machine:

```powershell
# elevated PowerShell
$hosts = "$env:windir\System32\drivers\etc\hosts"
Copy-Item $hosts "$hosts.bak-$(Get-Date -Format yyyyMMdd)"
cd C:\Users\mark_\Code\repos\genet\tests\wpt\tests
python wpt make-hosts-file | Add-Content -Encoding ascii $hosts
```

This appends ~64 loopback entries (`web-platform.test`, `not-web-platform.test`,
and their subdomains all to `127.0.0.1`); existing hosts entries are untouched, and
the backup is a clean revert. After this, `python wpt serve --exit-after-start`
binds every protocol (http/https/ws/wss/h2) and exits 0. Revert by restoring the
`.bak-<date>` copy (elevated).

**Gate B, server mode in `genet-wpt`. BUILT + PROVEN 2026-06-02.** Past the hosts
file, running the network-dependent `fetch/` tests needed harness changes, all
genet-side. They are done; see "Server mode" below for what shipped and the
results. The rest of this section records the server-side proof and the design.

### Server side proven (2026-06-02)

`python wpt serve` (plain-http on the stable port 8000) was stood up and probed
directly. All three behaviours that Gate B depends on work:

- **static serving**: `GET /common/blank.html` returns 200.
- **dynamic Python handlers**: `PUT /fetch/api/resources/method.py?show_request_method`
  echoes `x-request-method: PUT`; `inspect-headers.py?headers=x-test` with
  `x-test: hello` echoes `x-request-x-test: hello`.
- **template substitution** (the linchpin, and the thing disk-loading can never
  do): `GET /common/get-host-info.sub.js` comes back with real ports filled in
  (`HTTP_PORT = '8000'`, `HTTP_PORT2 = '62275'`), so `get_host_info()` /
  `make_absolute_url` will resolve correctly once the harness loads it over HTTP.

So the WPT server is solid. The rest was harness wiring, now done.

## Server mode (Gate B, shipped)

`genet-wpt testharness` gained a server mode behind the `netfetch` feature,
default off so disk-mode runs are byte-for-byte unchanged. Two ways in:

- `--server-base http://web-platform.test:8000` — connect to a `wpt serve` you
  started (it is probed once up front, so a typo / down server fails loudly).
- `--spawn-server` — the runner spawns `python wpt serve`, reads its primary
  plain-http port from the `... http on port N]` log line, waits until it answers,
  and tears the whole process tree down on exit.

What it wires (all on the `netfetch` feature):

1. **Fetch handler.** `net::NetFetchHandler` (netfetcher over one shared Tokio
   runtime) is installed on the per-test `Runtime` via `set_fetch_handler`. A ZST,
   so minting one per test is free.
2. **Base URL / `location`.** `Runtime::set_base_url` (new, in `script-runtime-api`)
   stores the test's document URL and populates `window.location` from its parsed
   components. A new `__resolve_url` native (over the `url` crate's `Url::join`)
   resolves relative `Request` / `fetch()` URLs against it; with no base set it is a
   no-op, so disk mode and the binding tests are unaffected.
3. **Server-loaded resources.** `collect_scripts` now takes a `ScriptSrcLoader`;
   the `net::ServerLoader` HTTP-GETs each `src` (joined against the document URL),
   so `get-host-info.sub.js` and other `.sub.js` arrive substituted. The test
   *page* itself is also GET from the server in server mode, so `.sub.html`
   substitution happens (`.any.js` wrappers are still synthesized locally). Disk
   mode uses `DiskLoader` (the prior `fs::read` path).
4. **Lifecycle.** `net::ServerCtx` (connect or spawn) owns the origin and a
   `ServerHandle` whose `Drop` kills the spawned tree (`taskkill /T /F` on Windows).

### Results (network fetch, was 0 — impossible — before)

A clean slice of `fetch/api/basic` (no `Blob`/`FormData`/stream deps), connect mode,
against a live `wpt serve`:

| test | boa | nova |
|---|---|---|
| http-response-code | 1/1 | 1/1 |
| mode-same-origin | 6/8 | 6/8 |
| accept-header | 3/4 | - |
| response-url.sub | 1/4 | - |

`http-response-code` is a full pass through the whole path: JS `fetch()` ->
`__fetch` -> `NetFetchHandler` -> netfetcher -> live WPT handler -> a `Response`
script asserts on. `accept-header`'s one failure is a real gap (genet sends no
`accept-language`), not a plumbing bug — the three Accept-header subtests that *do*
round-trip through `inspect-headers.py` pass.

### Run it

```bash
# connect mode (start the server yourself)
python wpt serve                                   # in tests/wpt/tests
cargo run -p genet-wpt --features netfetch -- \
  testharness fetch/api/basic --server-base http://web-platform.test:8000 --engine boa

# spawn mode (runner owns the server)
cargo run -p genet-wpt --features netfetch -- \
  testharness fetch/api/basic --spawn-server --engine boa
```

## The deferred fetch seam (the async switch, 2026-06-03)

The original seam was synchronous: `FetchHandler::fetch(req) -> FetchOutcome`, and
`genet-wpt` answered with `block_on`. That parks the JS thread inside the native
call, which structurally rules out mid-flight abort (no window for `abort()` to
run) and streaming bodies (no producer can deliver chunks while JS is blocked). It
is also the one place `genet` violated the message-passing discipline Mere's actor
plan ("fetch replies delivered as actor input, not direct netfetcher calls").

Inverted it to a **deferred push**, in three slices, all behind the existing
`netfetch` feature with zero new deps (the engine stays `!Send`):

1. **Seam.** `FetchHandler` gains `start(id, req) -> Option<FetchOutcome>` and
   `cancel(id)` alongside `fetch()`. The default `start` bridges to `fetch` and
   answers **inline** (`Some`), so a synchronous host (the offline mocks, the
   binding tests) resolves the Promise in the same tick, unchanged. A deferred host
   returns `None` and settles later. Rust cannot call a held JS function, so
   `Runtime::settle_fetch` / `fail_fetch` eval `__fetchSettle` / `__fetchFail` (the
   `__runTimers` shape) and pump, holding no `HostState` borrow across the eval. A
   JS `__pending` registry keyed by a monotonic id owns each `{resolve, reject}`
   with delete-once discipline; the pre-aborted path stays synchronous.
2. **Host + abort.** A single persistent worker thread owns the only Tokio runtime
   that touches netfetcher (current-thread + `spawn_blocking` job intake so the
   runtime thread stays free to drive in-flight fetches). Both blocking resource
   GETs and deferred `fetch()` route through it, so netfetcher's process-wide hyper
   client pool binds to a runtime that is always driven (a separate per-test
   runtime hung: the pool's IO was registered on a reactor nobody drove). Only
   plain owned data crosses `std::sync::mpsc`. `start` spawns the fetch; `cancel`
   aborts the task, **dropping the in-flight future** (drop-the-future
   cancellation). The harness drive loop resolves ready completions *before*
   advancing the cooperative timer clock (else `__runTimers`, which has no real-time
   gate, would fire the testharness timeout while a real fetch is in flight), and
   ends on quiescence or a per-test wall-clock deadline (so a hung fetch records
   TIMEOUT, never a hang). **Result:** the deferred path matches the sync path with
   no regression, and `fetch/api/abort/request` runs at 12/18 with real mid-flight
   abort.
3. **Streaming.** `ReadableStream.read()` gains a pending-waiter model (a live,
   not-yet-closed stream parks until the next chunk; buffered / closed streams never
   park, so their behaviour and the broken-then immunity of `text`/`json` are
   unchanged). `start_stream` early-settles with status + headers and a live body;
   `push_chunk` / `close_stream` feed it. Lifting the reader to a waiter model also
   closed part of the buffered-stream tail: network-free response 184 -> 196/290 on
   both engines.
4. **Runner streaming (follow-up, 2026-06-04).** The `genet-wpt` worker now polls
   the netfetcher body (`ResponseBody::next_chunk`, a small added method) and
   emits `StartStream` (status + headers) -> `Chunk`* -> `Close` instead of
   buffering the whole body. So `await fetch()` resolves at the headers and a
   mid-flight `controller.abort()` runs: `fetch/api/abort` went 37 -> **54/88** in
   server mode (`general` NORES -> 41/53, `keepalive`'s infinite-slow-response abort
   0 -> 1/2), both engines, with the network-free numbers (response 196, request
   257) and the basics (1/1, 6/8, 3/4) unchanged.

The seam is the actor-mailbox shape: a deferred handler owns a send into a worker
(or, in Mere, the I/O fetch actor's inbox), and replies arrive as messages that
drive `start_stream` / `push_chunk` / `close_stream` / `fail_fetch`. `genet-wpt`'s
handler is one consumer; Mere's content actor is the other, with no second refactor.

## HTTP cache (subsystem, 2026-06-04)

`request-cache-*` needs the client to honor the WHATWG request **cache mode**, not
just RFC 9111 default. netfetcher already had the RFC 9111 policy (freshness,
revalidation, 304 refresh) + `InMemoryHttpCache`; this added the mode:
`Request.cache` (`CacheMode`), the read/store/revalidate behaviour per mode, and
the mode-specific request headers (`Pragma`/`Cache-Control: no-cache`,
`Cache-Control: max-age=0`) that `cache.py` logs. `FetchContext.cache` became an
`Arc` so the runner shares **one** process-wide cache across fetches; the mode
crosses the deferred seam (`FetchRequest.cache` -> `__fetch_start` -> the worker).
A `reason_phrase()` map fills `response.statusText` (netfetcher discards the wire
reason), which every cache subtest checks.

request-cache, server mode (was ~2): **default 8/8, force-cache 16/16, no-cache
4/4, no-store 8/8, reload 12/12, only-if-cached 10/14, default-conditional 20/40**
(~+76). Remaining: cache + redirect interaction and cross-origin-redirect edges.

## Not done (deliberately deferred)

- **Byte (BYOB) byte streams.** `getReader({mode:'bytes'})` returns a default
  reader (the view-passing tests pass through it leniently); a true byte stream
  with BYOB `read(view)` is unbuilt. Few subtests need it.
- **`request/destination` + multi-realm.** `request/destination` (~48) needs the
  DOM to initiate fetches with a destination (an `<img>` -> `image`, etc.) — a
  document/element-fetch integration the runner has no model for; `*/url-parsing.html`
  and the realm tests need iframe / multi-global support. Both are larger
  subsystems (element-driven fetch, a second realm), not fetch-surface fixes.
- **Cache + redirect.** `default-conditional`'s back half and `only-if-cached`'s
  cross-origin-redirect cases need caching to compose with redirect following
  (cache the redirect, key the redirected URL).
- **Strict malformed-multipart rejection.** `formData()` parses valid multipart
  (round-trips), but does not reject the buggy-form-data inputs; a binary `File`
  part still goes through UTF-8 text (the one lossy body spot).
- **Per-test runtime reuse.** A fresh `Runtime` per test re-evals testharness.js
  each time (the dominant cost; see `harness::bench`). A snapshot-clone pool is the
  amortization, unchanged by this work.

## Pointers

- Seam + surface: `components/script-runtime-api/fetch.rs`, `lib.rs`
  (`set_base_url`, `__resolve_url`); tests `tests/fetch_binding.rs` (7).
- Server mode: `ports/genet-wpt/src/main.rs` `mod net` (`ServerCtx`,
  `ServerLoader`, `NetFetchHandler`, `parse_http_port`) + `setup_server`; the
  `ScriptSrcLoader` / `DiskLoader` split is in `src/harness.rs`. Unit tests:
  `net::tests` (port parse, doc-url join). Off-server proof: `tests/fetch_netfetcher.rs`.
- Runner wrapping: `ports/genet-wpt/src/main.rs` (`synthesize_any_js`).
- netfetcher refinements behind this (PSL same-site, h3-with-body, mixed-content
  split): netfetcher commit `8bde3c1`.
- This work: genet `c98f551aeb7` (richer API + wrapping), `c1c9246beeb` (seam),
  plus the server-mode commit recorded alongside this doc.
