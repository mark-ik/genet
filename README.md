# netfetcher

A portable, off-UI-thread **WHATWG Fetch** network engine for the Mere ecosystem.
It is Servo's `net` made portable: the Fetch algorithm (CORS, cookie jar, HTTP
cache, redirects, HSTS, mixed-content upgrade, CSP hooks, content-encoding,
Subresource Integrity, referrer policy) lifted off Servo's `ipc-channel` /
resource-thread coupling and exposed as a directly callable async **library**,
plus an HTTP/3 lane and a modern rustls stack.

netfetcher is a network *organ*: a sibling crate the Mere host drives on a
background tokio runtime and hands bytes back to renderers. It covers http(s),
ws(s), and data: URLs. Smolweb protocols are a separate engine
([`errand`](https://github.com/mark-ik/errand)); the host's loader actor routes
http(s) to netfetcher and smolweb schemes to errand.

**Made with AI**

- Package: `netfetcher` (single crate, not a workspace)
- Version: `0.0.0` (`publish = false`)
- Edition: 2024
- License: MPL-2.0 (lineage from Servo's `net`)
- Repository: <https://github.com/mark-ik/netfetcher>

## What it is for

The Mere host owns networking. It runs netfetcher off the UI thread (in a
fetcher-pool worker) and feeds resulting bytes to render engines. Renderers stay
byte-consuming: the JS `fetch()` binding calls netfetcher *through the host*
rather than by linking it directly into the renderer's core.

Consumers, both via a `git`/`branch` dependency on this repo:

- `mere/crates/meerkat`: a non-optional dependency. Its loader actor routes
  http(s) to netfetcher and smolweb schemes to errand, running the async fetch
  on a tokio runtime and waking the UI via an `EventLoopProxy`.
- `genet` ports (`pelt-desktop`, `genet-wpt`): an **optional** dependency
  behind a `netfetch` feature (off by default), used to back the runtime's
  `FetchHandler` seam so `fetch()` and remote `static https://...` documents
  load over this engine. The genet engine components themselves stay
  byte-consuming.

The dependency direction is one-way: Mere and genet consume netfetcher;
netfetcher does not depend on them.

## Status

Implemented and unit-tested through the planned v1 ladder, plus refinements
landed since the original increment work. The module-level doc comment in
`src/lib.rs` is the authoritative status; the increment summary there:

- **1** h1/h2 GET/POST over hyper + rustls; redirects (follow / error / manual);
  streaming bodies with on-the-fly `Content-Encoding` decode (gzip, deflate,
  brotli, zstd).
- **2** RFC 6265bis cookie jar; RFC 9111 HTTP cache (freshness + `ETag` /
  `Last-Modified` revalidation) over pluggable storage.
- **3** cross-origin model: response tainting (`Basic` / `Cors` / `Opaque`),
  CORS (simple + preflight + response-header filtering, with a preflight cache),
  HSTS, mixed-content auto-upgrade, SameSite; a CSP `connect-src` hook.
- **4** HTTP/3 via Alt-Svc: a transport-abstracted h3 lane (quinn + h3) with
  h1/h2 fallback.
- **5** WebSocket (`ws://` / `wss://`).

Refinements landed since the increment ladder (from git history):

- `data:` URL processor (WHATWG) with a minimal MIME parser/serializer.
- Subresource Integrity verification (sha256 / sha384 / sha512, strongest-present
  wins; standard or URL-safe base64).
- Referrer engine: `Referer` header computation under W3C Referrer Policy,
  recomputed per redirect hop.
- Default `User-Agent`; `Content-Length: 0` for bodyless POST/PUT; `Origin` on
  unsafe methods.
- Arbitrary HTTP method tokens (`Method::Other`).
- Opt-in `accept-invalid-certs` for local test harnesses.
- Range request handling and cache-on-read streaming of cacheable responses.

Native-focused. The h3 and WebSocket lanes are native-only (excluded from wasm
builds via `#[cfg(not(target_arch = "wasm32"))]`); a wasm build would bind the
browser's `fetch` / `WebSocket`.

Deferred: h3 for requests with bodies, the active/passive mixed-content split,
and remaining same-site edge cases (the engine carries the `psl` crate for
registrable-domain accuracy). Conformance against the WPT `fetch/` suite is not
yet wired; unit tests use an offline mock server (`mockito`) and in-process test
servers.

## Usage

```rust
let req = netfetcher::Request::get("https://example.org/".parse()?);
let cx  = netfetcher::FetchContext::permissive();
let res = netfetcher::fetch(req, &cx).await;   // real h1/h2/h3 response
```

`Request` carries Fetch-algorithm concepts rather than wire-level `http` types:
`url`, `method`, `headers`, `body`, `mode`, `credentials`, `redirect`, `origin`,
`destination`, `cache`, `referrer`, `referrer_policy`, and `integrity`. The
response body is a `ResponseBody` stream of decoded chunks; `Response::bytes`
collects the whole thing.

`FetchContext` is the caller-owned bundle of policy and pluggable storage that
the Fetch algorithm threads through (cookie jar, HTTP cache, HSTS store, Alt-Svc
store, preflight cache, CSP checker). Storage is behind traits so the host can
back it with persona- or session-scoped partitions; the seams take `&self` and
use interior mutability, so one shared `&FetchContext` can both read and record
during a fetch and is `Send + Sync`.

### Public API

Re-exported from `src/lib.rs`:

- Entry point: `fetch`.
- Request types: `Request`, `Method`, `RequestMode`, `Credentials`,
  `RedirectMode`, `CacheMode`, `Destination`, `ReferrerPolicy`.
- Response types: `Response`, `ResponseBody`, `ResponseType`.
- Context and seams: `FetchContext`, `CookieStore`, `CspChecker`, `AllowAllCsp`,
  `SameSiteContext`.
- Storage backends: `InMemoryCookieJar`, `HttpCache`, `InMemoryHttpCache`,
  `NoHttpCache`, `StoredResponse`, `HstsStore`, `InMemoryHsts`, `AltSvcStore`,
  `InMemoryAltSvc`, `PreflightCache`, `InMemoryPreflightCache`.
- Test seam: `accept_invalid_certs`.
- WebSocket (native-only): `WebSocket`, `WsMessage`, `connect_websocket`.

## Module map

All source lives under `src/` (single-crate library, no workspace members).

| Module | Role |
| --- | --- |
| `lib.rs` | Crate root, module declarations, public re-exports, status doc. |
| `fetch.rs` | The Fetch entry point and main algorithm (largest module). |
| `request.rs` | The Fetch-spec `Request` type and builder. |
| `response.rs` | The Fetch-spec `Response` and the streaming `ResponseBody`. |
| `context.rs` | `FetchContext` and the pluggable policy/storage seams. |
| `client.rs` | The shared hyper client (connection pool + rustls TLS). |
| `cors.rs` | Response tainting, CORS simple/preflight, preflight cache. |
| `cookie_jar.rs` | RFC 6265bis cookie jar. |
| `cache.rs` | RFC 9111 HTTP cache (freshness + revalidation). |
| `hsts.rs` | HSTS store and upgrade logic. |
| `referrer.rs` | `Referer` computation under W3C Referrer Policy. |
| `decode.rs` | On-the-fly `Content-Encoding` decode stream. |
| `sri.rs` | Subresource Integrity verification. |
| `data_url.rs` | `data:` URL processing + minimal MIME parser. |
| `altsvc.rs` | Alt-Svc store for HTTP/3 discovery. |
| `h3_client.rs` | HTTP/3 transport over QUIC (quinn + h3); native-only. |
| `websocket.rs` | WebSocket over `ws://` / `wss://`; native-only. |

## Build, run, and test

Standard Cargo; no special toolchain, build script, or environment needed.

```sh
cargo build            # build the library
cargo build --release  # release build
cargo test             # run the test suite
```

There are roughly 65 unit/integration tests across the modules
(`#[test]` and `#[tokio::test]`), heaviest in `fetch.rs` (about 17) and
`cors.rs` (about 13). Tests run offline against `mockito` and in-process test
servers (the h3 round-trip test stands up an in-process quinn h3 server with an
`rcgen` self-signed cert). There are no `[[bin]]` targets or `examples/`.

## Dependency stack

Key pins from `Cargo.toml`:

- Transport: `hyper` 1 (client, http1, http2), `hyper-util` 0.1 (legacy client /
  connection pool), `http` 1, `http-body-util` 0.1.
- TLS: `hyper-rustls` 0.27 (webpki-roots, ring, tls12), `rustls` 0.23 (ring, std,
  tls12).
- Crypto for SRI: `ring` 0.17, `base64` 0.22.
- Async: `tokio` 1 (multi-thread runtime), `futures-util` 0.3, `tokio-util` 0.7.
- Body decode: `async-compression` 0.4 (gzip, zlib, brotli, zstd).
- Cookies and dates: `cookie` 0.18, `time` 0.3, `httpdate` 1.
- Public Suffix List: `psl` 2.
- HTTP/3: `quinn` 0.11, `h3` 0.0.8, `h3-quinn` 0.0.10, `webpki-roots` 1. h3 is
  pre-1.0; API churn is expected.
- WebSocket: `tokio-tungstenite` 0.29 (rustls-tls-webpki-roots).
- Dev: `mockito` 1, `flate2` 1 (gzip test fixtures), `rcgen` 0.14 (self-signed
  cert for the in-process h3 test server).

## Design and plan

The full design (scope, the de-IPC extraction strategy, the dependency stack,
layering, and open questions) lives in the Mere workspace, which owns it:

> `mere/design_docs/archive_docs/2026-06-09_completed_plans/2026-05-25_netfetcher_plan.md`
> (the plan is complete; it was moved here from
> `implementation_strategy/`, which is the path still cited in `src/lib.rs`).

Open question #1 (whether `Request` thin-wraps the `http` crate instead of owning
its own types) is deferred; for now netfetcher owns its Fetch types.

## License

MPL-2.0 (lineage from Servo's `net`).
