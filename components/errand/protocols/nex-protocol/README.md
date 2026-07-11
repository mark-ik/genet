# nex-protocol

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/errand/protocols/nex-protocol` (adopted 2026-07). The former standalone repository is archived
> and links here.


A Rust implementation of the Nex protocol (`nex://`): an async client, a
directory-serving server, a listing parser, and a small CLI.

Nex, from [nightfall.city](https://nightfall.city/nex/), is the minimal
smolweb protocol, inspired by gopher and gemini. The whole wire format: the
client connects to TCP port 1900 and sends a path (which may be empty); the
server responds with text or binary data and closes the connection. No TLS,
no status codes, no headers, no state. The spec lives at
`nex://nightfall.city/nex/info/specification.txt`.

This crate is independent and unaffiliated with the protocol's author. The
crates.io name is qualified (`nex-protocol`) because the bare `nex` name is
used by an unrelated project.

## Library

```rust
use nex_protocol::{fetch, parse_listing, ListingLine, FetchOptions};

let body = fetch("nex://nightfall.city/", &FetchOptions::default()).await?;
for line in parse_listing(&String::from_utf8_lossy(&body)) {
    if let ListingLine::Link { url, .. } = line {
        println!("→ {url}");
    }
}
```

Serving is a `Handler` (any async closure) plus `serve`; `FileHandler` serves
a directory tree — `index.nex` per directory when present, else a generated
`=> ` listing — with traversal protection.

## CLI

```sh
cargo install nex-protocol --features cli

nex fetch nex://nightfall.city/
nex serve --root ./site --listen 0.0.0.0:1900
```

## Spec coverage

| Spec point | State |
| --- | --- |
| Selector request (path, may be empty), response = bytes until close | client + server |
| Port 1900 | default both sides |
| Directory listing format (`=> ` link lines, absolute or relative URLs) | parser + generated listings |
| Empty path / trailing `/` = directory | `is_directory_path`, honored by `FileHandler` |
| Extension-based display, plain text default | left to the consumer (a transport crate doesn't render); `FileHandler` serves bytes verbatim |

Notes on choices the spec leaves open: link-line labels (text after the URL)
are preserved as a client convention; the selector line accepts bare LF as
well as CRLF (the spec's own example is a telnet session); over-long
selectors just close the connection, since nex has no error channel.

## License

MIT. The specification text declares no license; this implementation is
original code.
