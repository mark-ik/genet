# guppy-protocol

> **Home:** [`mark-ik/genet`](https://github.com/mark-ik/genet), at
> `components/errand/protocols/guppy-protocol` (adopted 2026-07). The former standalone repository is archived
> and links here.


A Rust implementation of the
[Guppy protocol](https://github.com/dimkr/guppy-protocol) v0.4.4
(`guppy://`): an async UDP client and server with chunking, per-packet
acknowledgement, windowed transmission, and retransmission, plus a small CLI.

Guppy is dimkr's smolweb-over-UDP protocol, inspired by TFTP, DNS, and
Spartan, designed to be servable from a microcontroller over a single UDP
socket (default port 6775). A request is one datagram carrying a URL; the
response is a numbered packet sequence the client acknowledges packet by
packet, ended by an empty end-of-file packet. User input rides the URL query,
prompted by a `1 <prompt>` packet; `3` redirects and `4` errors are
single-packet answers.

This crate is independent and unaffiliated with the protocol's author. The
crates.io name is qualified (`guppy-protocol`) because the bare `guppy` name
is used by an unrelated project.

## Library

```rust
use guppy_protocol::{GuppyResponse, fetch, FetchOptions};

match fetch("guppy://guppy.mozz.us/", &FetchOptions::default()).await? {
    GuppyResponse::Success { mime, body } => { /* render */ }
    GuppyResponse::Prompt { text } => { /* re-request with ?input */ }
    GuppyResponse::Redirect { target } => { /* user-confirmed re-request */ }
    GuppyResponse::Error { message } => { /* show it; errors are for users */ }
}
```

Serving is a `Handler` (any async closure) returning the same
`GuppyResponse`; the server owns chunking, the send window, retransmission,
session tracking, and duplicate-request suppression. `FileHandler` is the
built-in static-directory handler.

## CLI

```sh
cargo install guppy-protocol --features cli

guppy fetch guppy://guppy.mozz.us/
guppy serve --root ./site --listen 0.0.0.0:6775
```

## Spec coverage (v0.4.4)

| Spec point | State |
| --- | --- |
| Request = one datagram `url\r\n`, ≤ 2048 bytes | enforced both sides |
| Success / continuation / EOF packet forms, seq +1, random start in [6, 2³¹−1] | full; the "don't confuse seq 39/41 with 3/4" disambiguation is unit-tested |
| Per-packet acknowledgement; duplicate packets re-acked; duplicate acks ignored | client + server |
| Retransmission (request, acks, unacked response packets) | both sides, tunable intervals |
| Out-of-order transmission | server sends a configurable window ahead of acks; client caches and re-orders (tested against scrambled + duplicated delivery) |
| Sessions (source-addr keyed, duplicate requests ignored, timeout, DoS cap) | server, with a post-completion linger |
| Input prompts (`1`), redirects (`3`), errors (`4`) | full round trip; input decodes from the URL query |
| Chunk size ≥ 512 recommendation | default 512, configurable |
| One source port per session | client binds a fresh socket per fetch |

Not covered: encryption (the spec declares it out of scope).

## License

MIT. The specification declares no license; this implementation is original
code.
