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
outcome as hand-rolled JSON (no JSON dep). No handler installed → network error,
which is the spec-correct default.

### 2. The JS Fetch API surface (`FETCH_BOOTSTRAP`)

Bootstrapped in JS over the single `__fetch` sink:

- **Headers** — RFC 7230 name validation (token regex), OWS-trimmed values,
  `append`/`set`/`get`/`has`/`delete`/`getSetCookie`, sorted iteration
  (`entries`/`keys`/`values`/`Symbol.iterator`/`forEach`).
- **Request** — full object, `GET`/`HEAD`+body throws, `clone`.
- **Response** — `new Response(body, init)`, status-range check, `error`/
  `redirect`/`json` statics, `clone`.
- **Body mixin** — `text`/`json`/`arrayBuffer`, single-use via a consumed flag.
- **`fetch()`** — takes a Request, adds a default `Accept`, joins headers with
  `"\n"`, builds a Response from the outcome.

Separator is a literal newline on both sides (JS `join("\n")`, Rust `split('\n')`).

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

All three were **0** before this work (the files would not even parse/run). These
are the headers/request/response object-semantics tests that need no live server.
Boa↔nova agree on headers and response; request differs only in a subtest-**count**
tail (469 vs 429 enumerated), same pass count.

## The `wpt serve` lift: blocked, two gates

The full WPT server tooling is vendored (`tests/wpt/tests/` has the `wpt` CLI,
`tools/serve/serve.py`, `tools/certs/`, the `fetch/` Python handlers). Python
3.14.2 is present; `wpt serve --exit-after-start` bootstraps its venv and deps
cleanly. It then fails:

```
CRITICAL - start_http_server: getaddrinfo failed
Please ensure all the necessary WPT subdomains are mapped to a loopback device.
```

**Gate A — hosts file.** `wpt serve` resolves `web-platform.test` and its
subdomains for its own readiness probe. A localhost-only `--config` override does
not sidestep this (`subdomains` is not even a valid config-override key —
`KeyError`). The fix is the documented one-time admin step: append
`wpt make-hosts-file` output to `C:\Windows\System32\drivers\etc\hosts`. This needs
the user's elevation; it is not something the harness can do unattended.

**Gate B — server mode in `serval-wpt`.** Past the hosts file, running the
network-dependent `fetch/` tests needs the runner to load server-served and
template-substituted pages and set the running server as the document base URL, so
relative `fetch()` calls resolve to it. That is a substantial harness rework beyond
the netfetcher handler already built.

## Not done (deliberately deferred)

- **`wpt serve` stand-up** — gated on Gate A (user admin step) then Gate B
  (server-mode harness). The netfetcher handler is ready to drive it once both are
  cleared.
- **The failing object-semantics tail** — request/response sit well under half.
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
