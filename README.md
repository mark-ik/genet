# netfetcher

A portable **WHATWG-Fetch** network engine for the Mere ecosystem — Servo's `net`
*made portable*: the Fetch algorithm (CORS, cookie jar, HTTP cache, redirects,
HSTS, mixed-content, CSP hooks, content-encoding) lifted off Servo's
`ipc-channel` / resource-thread coupling and exposed as a directly-callable async
**library**, plus an HTTP/3 lane and a modern rustls stack.

A network *organ* sibling to [`serval`](../serval) (render engine) and
[`netrender`](../netrender) (paint→GPU). **Mere owns networking; consumers
receive bytes** — serval and other renderers never link netfetcher; the JS
`fetch()` binding calls it through the host.

## Status — increments 1–5 (2026-05-26)

The planned v1 ladder is implemented and tested (see [`src/lib.rs`] for the
authoritative module-level status):

- **1** h1/h2 GET/POST over hyper + rustls, redirects (follow/error/manual),
  streaming bodies with on-the-fly `Content-Encoding` decode.
- **2** RFC 6265bis cookie jar; RFC 9111 cache (freshness + `ETag`/`Last-Modified`
  revalidation), pluggable storage.
- **3** cross-origin model: response tainting (`Basic`/`Cors`/`Opaque`), CORS
  (simple + preflight + response-header filtering), HSTS, mixed-content
  auto-upgrade, SameSite; the CSP `connect-src` hook.
- **4** HTTP/3 via Alt-Svc — a transport-abstracted h3 lane (quinn) with h1/h2
  fallback.
- **5** WebSocket (`ws://` / `wss://`).

The h3 and WebSocket lanes are native-only (wasm-excluded). Deferred refinements:
h3 for requests with bodies, the active/passive mixed-content split, and
public-suffix-accurate same-site. Conformance against the WPT `fetch/` suite is
not yet wired (unit tests use an offline mock server).

```rust
let req = netfetcher::Request::get("https://example.org/".parse()?);
let cx  = netfetcher::FetchContext::permissive();
let res = netfetcher::fetch(req, &cx).await;   // real h1/h2/h3 response
```

## Plan

The full design — scope, de-IPC extraction strategy, dependency stack, layering,
and open questions — lives in the Mere workspace (Mere owns it):

> `mere/design_docs/mere_docs/implementation_strategy/2026-05-25_netfetcher_plan.md`

The increment ladder (core GET → cookies+cache → CORS/CSP/HSTS/mixed-content →
HTTP/3 → WebSocket) is implemented; see the Status section above. Conformance
oracle: Servo `net` byte-diff (early) + the WPT `fetch/` suite (not yet wired).

## License

MPL-2.0 (lineage from Servo's `net`).
