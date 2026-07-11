# spartan-protocol

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/errand/protocols/spartan-protocol` (adopted 2026-07). The former standalone repository is archived
> and links here.


A spec-faithful Rust implementation of the
[Spartan protocol](https://github.com/michael-lazar/spartan) (`spartan://`):
an async client, a server with a pluggable handler, static file serving, and
a small CLI. 💪

Spartan is Michael Lazar's plaintext smolweb protocol: ASCII requests over
TCP (default port 300), arbitrary uploads and downloads, gemtext as the
preferred document format with a `=:` prompt line for input, and four
single-digit status codes.

This crate is independent and unaffiliated with the protocol's author. The
crates.io name is qualified (`spartan-protocol`) because the bare `spartan`
name is used by an unrelated project; if the protocol community ever wants
this name coordinated differently, open an issue.

## Library

```rust
use spartan_protocol::{Status, fetch, submit, FetchOptions};

// Fetch (query components upload as the data block, per spec §5).
let response = fetch("spartan://spartan.mozz.us/", &FetchOptions::default()).await?;

// Upload (the `=:` prompt flow).
let reply = submit("spartan://example.com/guestbook", b"Hello!", &FetchOptions::default()).await?;
```

Serving is a `Handler` (any async closure) plus `serve`; `FileHandler` is the
built-in static-directory handler (gemtext-first MIME table, `index.gmi` per
directory, %-decode traversal protection, uploads refused):

```rust
use spartan_protocol::{FileHandler, ServerConfig, serve};

let listener = tokio::net::TcpListener::bind("0.0.0.0:300").await?;
serve(listener, FileHandler::new("/srv/spartan"), ServerConfig::default(), shutdown).await?;
```

## CLI

```sh
cargo install spartan-protocol --features cli

spartan fetch spartan://spartan.mozz.us/
spartan submit spartan://example.com/guestbook "Hello world!"
spartan serve --root ./site --listen 0.0.0.0:3000
```

## Spec coverage (2021-03-24 revision)

| Spec section | State |
| --- | --- |
| §2 request line (`host SP path SP content-length CRLF`) + data block | client + server, upload limits configurable |
| §3 responses (`2`/`3`/`4`/`5` + META, body on success only) | full |
| §4.1 gemtext `=:` prompt line | `parse_prompt_line` helper; `submit` is the send half |
| §5 URL mapping (default port 300, IDN → punycode, %-encoding, query → data block) | full, tested against the spec's reference table (one erratum found: the table prints `xn--exampl-dma.com` for `examplé.com`, but IDNA yields `xn--exampl-gva.com`; `dma` is the café example's suffix) |
| §3 same-host redirect rule | surfaced (`redirect_path`); following is the caller's decision |

## License

MIT. The spec repository declares no license for the specification text; this
implementation is original code.
