# WPT fetch/ conformance: JS surface, netfetcher binding, serve lift

Status: **2026-06-02.** Stood up the JS Fetch API surface, wired `fetch()` to a
host seam, backed that seam with netfetcher in `serval-wpt`, and taught the
runner to wrap `.any.js` tests. The network-free `fetch/api/` subset runs and
scores on both engines (was 0 before). Both gates are now cleared: the hosts file
is set up (Gate A) and `serval-wpt` has a **server mode** (Gate B) that drives the
network-dependent `fetch/` corpus against a live `wpt serve`. Network fetch tests
that were previously impossible now pass against the real server on both engines
(e.g. `fetch/api/basic/http-response-code` 1/1, `mode-same-origin` 6/8).

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

### 3. netfetcher-backed handler (`ports/serval-wpt`, `netfetch` feature)

`NetFetchHandler` holds a current-thread tokio runtime and implements
`FetchHandler` by `block_on`-ing `netfetcher::fetch`, mapping Method, response
type, final URL (`url_list.last()`), and body bytes. Gated behind the optional
`netfetch` feature so the default `serval-wpt` build stays free of the async net
stack (confirmed unaffected).

### 4. `.any.js` wrapping in the runner (`src/main.rs`)

`is_any_js` matches `.any.js`/`.window.js`/`.worker.js`; `synthesize_any_js`
parses `// META: script=` and `global=` directives and builds a wrapper HTML
(testharness.js + meta scripts + the file). Worker-only globals are skipped. This
is what makes the `fetch/api/` `.any.js` corpus runnable at all.

## Network-free numbers (both engines, release `serval-wpt`)

| subset | boa subtests | nova subtests |
|---|---|---|
| fetch/api/headers | 86/197 | 86/197 |
| fetch/api/request | 257/545 | 257/505 |
| fetch/api/response | 182/290 | 182/290 |

All three were **0** before this work (the files would not even parse and run).
These are the headers/request/response object-semantics tests that need no live
server. Boa and nova agree on pass counts everywhere; request differs only in a
subtest-**count** tail (545 vs 505 enumerated).

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
  Byte (BYOB) readers, `pipeTo` / `pipeThrough`, and true async producers are still
  deferred.
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
cd C:\Users\mark_\Code\repos\serval\tests\wpt\tests
python wpt make-hosts-file | Add-Content -Encoding ascii $hosts
```

This appends ~64 loopback entries (`web-platform.test`, `not-web-platform.test`,
and their subdomains all to `127.0.0.1`); existing hosts entries are untouched, and
the backup is a clean revert. After this, `python wpt serve --exit-after-start`
binds every protocol (http/https/ws/wss/h2) and exits 0. Revert by restoring the
`.bak-<date>` copy (elevated).

**Gate B, server mode in `serval-wpt`. BUILT + PROVEN 2026-06-02.** Past the hosts
file, running the network-dependent `fetch/` tests needed harness changes, all
serval-side. They are done; see "Server mode" below for what shipped and the
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

`serval-wpt testharness` gained a server mode behind the `netfetch` feature,
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
script asserts on. `accept-header`'s one failure is a real gap (serval sends no
`accept-language`), not a plumbing bug — the three Accept-header subtests that *do*
round-trip through `inspect-headers.py` pass.

### Run it

```bash
# connect mode (start the server yourself)
python wpt serve                                   # in tests/wpt/tests
cargo run -p serval-wpt --features netfetch -- \
  testharness fetch/api/basic --server-base http://web-platform.test:8000 --engine boa

# spawn mode (runner owns the server)
cargo run -p serval-wpt --features netfetch -- \
  testharness fetch/api/basic --spawn-server --engine boa
```

## Not done (deliberately deferred)

- **`WritableStream` / `TransformStream` + byte/async streams.** The
  `ReadableStream` is a buffered model with correct body lock/disturb semantics,
  but `pipeTo` / `pipeThrough` (need `WritableStream` / `TransformStream`), byte
  (BYOB) readers, and genuinely async producers are deferred:
  `response-stream-disturbed-by-pipe` (0/2) and the back-4 of `disturbed-5` need
  them.
- **Multipart bodies.** `formData()` parses urlencoded but not multipart, and a
  binary `File` part in a multipart request body is still spliced as text (the one
  remaining lossy body spot). A multipart parser/serializer over the byte body is
  the fix. (The general binary body channel is now done; see above.)
- **Live mid-flight abort.** `fetch()` runs synchronously through `block_on`, so
  an `AbortController.abort()` *after* the call cannot interrupt it. Only the
  pre-flight abort check works. The bulk of `fetch/api/abort/general` asserts
  mid-flight interruption, so it stays around 25/53 even with a server (and is
  slow there: it is `timeout=long` and does ~50 sequential fetches).
- **`iframe` / `contentWindow`.** `*/url-parsing.html` and the multi-global tests
  reach into iframe globals, which the single-realm runner has no model for.
- **The failing object-semantics tail**: request / response still sit around half,
  now mostly byte/async streams + mid-flight abort + iframes + multipart, not
  missing constructors or a lossy body channel.
- **Per-test runtime reuse.** A fresh `Runtime` per test re-evals testharness.js
  each time (the dominant cost; see `harness::bench`). A snapshot-clone pool is the
  amortization, unchanged by this work.

## Pointers

- Seam + surface: `components/script-runtime-api/fetch.rs`, `lib.rs`
  (`set_base_url`, `__resolve_url`); tests `tests/fetch_binding.rs` (7).
- Server mode: `ports/serval-wpt/src/main.rs` `mod net` (`ServerCtx`,
  `ServerLoader`, `NetFetchHandler`, `parse_http_port`) + `setup_server`; the
  `ScriptSrcLoader` / `DiskLoader` split is in `src/harness.rs`. Unit tests:
  `net::tests` (port parse, doc-url join). Off-server proof: `tests/fetch_netfetcher.rs`.
- Runner wrapping: `ports/serval-wpt/src/main.rs` (`synthesize_any_js`).
- netfetcher refinements behind this (PSL same-site, h3-with-body, mixed-content
  split): netfetcher commit `8bde3c1`.
- This work: serval `c98f551aeb7` (richer API + wrapping), `c1c9246beeb` (seam),
  plus the server-mode commit recorded alongside this doc.
