# WPT fetch/ conformance: JS surface, netfetcher binding, serve lift

Status: **2026-06-02.** Stood up the JS Fetch API surface, wired `fetch()` to a
host seam, backed that seam with netfetcher in `serval-wpt`, and taught the
runner to wrap `.any.js` tests. The network-free `fetch/api/` subset now runs and
scores on both engines (was 0 before). The full network-dependent run is gated on
`wpt serve`, which is blocked on a one-time hosts-file setup plus a server-mode
harness rework.

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
| fetch/api/request | 168/469 | 168/429 |
| fetch/api/response | 46/199 | 46/199 |

All three were **0** before this work (the files would not even parse and run).
These are the headers/request/response object-semantics tests that need no live
server. Boa and nova agree on headers and response; request differs only in a
subtest-**count** tail (469 vs 429 enumerated), with the same pass count.

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

**Gate B, server mode in `serval-wpt`.** Past the hosts file, running the
network-dependent `fetch/` tests needs harness changes. This is the remaining work;
it is all serval-side, no further external setup.

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

So the WPT server is solid. The rest is harness wiring.

### Harness changes (grounded in `harness.rs` + `main.rs`)

Today `harness::run_test` reads the test HTML from disk, `collect_scripts` reads
inline + local `<script src>` from disk (remote/`data:` skipped), no `fetch`
handler is installed on the `Runtime`, and there is no document URL / `location`.
Add a **server mode** behind a new flag (`--server-base http://web-platform.test:8000`),
default off so disk mode is unchanged:

1. **Install the fetch handler.** In `run_with`, when in server mode,
   `rt.set_fetch_handler(Box::new(NetFetchHandler::new()))` — promote the handler
   from `tests/fetch_netfetcher.rs` into the binary behind the `netfetch` feature.
   Test `fetch()` calls then hit the live server.
2. **Base URL / `location`.** Set the document URL to `<server-base>/<test-rel-path>`
   so relative `fetch()` and `make_absolute_url` resolve. Two parts: a host
   `location` global (`href`/`origin`/`protocol`/`host`/`pathname`), and relative
   URL resolution in `fetch()` (best as a native fn over the Rust `url` crate's
   `Url::join`, not a JS reimplementation).
3. **Server-loaded resources (the bulk).** In server mode, `collect_scripts` loads
   `<script src>` (and the test HTML itself when `.sub.html`) by HTTP GET from the
   server instead of `fs::read_to_string`, so `.sub.js`/`.sub.html` substitution
   happens. `get-host-info.sub.js` then carries real ports (proven above).
4. **Server lifecycle.** Two options: (a) **connect mode** — user runs
   `wpt serve`, passes `--server-base`; simplest, recommended first. (b)
   **auto-spawn** — runner spawns `wpt serve`, parses the http port from its
   `Starting http server on http://web-platform.test:PORT` line (8000 is the stable
   plain-http port), waits for ready, tears down. More convenient, more moving
   parts; later.

Dependencies: (1) and (3) need a running server (4); (2) is independent; (3) is
most of the work. First slice: one network `fetch/` test end-to-end in connect
mode (handler + base URL + server-loaded `get-host-info`), confirm its subtests
pass against the live server, then widen.

## Not done (deliberately deferred)

- **Gate B server mode**: the four-step harness rework above. Server side proven;
  harness wiring pending a go-ahead on connect-vs-spawn.
- **The failing object-semantics tail**: request/response sit well under half.
  Many failures are missing pieces (`FormData`, `Blob`, `URLSearchParams` bodies,
  `ReadableStream`), not seam bugs. Each is its own slice.

## Pointers

- Seam + surface: `components/script-runtime-api/fetch.rs`, `lib.rs`; tests
  `tests/fetch_binding.rs` (7).
- netfetcher handler: `ports/serval-wpt` `netfetch` feature; test
  `tests/fetch_netfetcher.rs` (mockito).
- Runner wrapping: `ports/serval-wpt/src/main.rs` (`synthesize_any_js`).
- netfetcher refinements behind this (PSL same-site, h3-with-body, mixed-content
  split): netfetcher commit `8bde3c1`.
- This work: serval `c98f551aeb7` (richer API + wrapping), `c1c9246beeb` (seam).
