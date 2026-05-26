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

## Status — scaffold (2026-05-25)

Repo skeleton only. The public API shape compiles and is exercisable, but
[`fetch`] returns a Fetch-spec **network error** — nothing is wired yet.

```rust
let req = netfetcher::Request::get("https://example.org/".parse()?);
let cx  = netfetcher::FetchContext::permissive();
let res = netfetcher::fetch(req, &cx).await;   // network error, for now
```

## Plan

The full design — scope, de-IPC extraction strategy, dependency stack, layering,
and open questions — lives in the Mere workspace (Mere owns it):

> `mere/design_docs/mere_docs/implementation_strategy/2026-05-25_netfetcher_plan.md`

### Increment ladder (ordered by policy depth)

1. **Core GET + plumbing** — h1/h2 via hyper + hyper-rustls, redirects,
   content-encoding, the `fetch()` entry + streaming body, basic cookie jar.
2. **Cookies + cache** — RFC 6265bis jar, RFC 9111 cache, pluggable storage.
3. **CORS + CSP hook + HSTS + mixed-content** — preflight, tainting, the seams.
4. **HTTP/3** — quinn + h3 behind Alt-Svc discovery.
5. **WebSocket** (optional) — gated on real demand.

Conformance oracle: Servo `net` byte-diff (early) + the WPT `fetch/` suite.

## License

MPL-2.0 (lineage from Servo's `net`).
