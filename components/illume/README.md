# illume

A portable, pure-Rust, wasm-safe text lexer and syntax highlighter.

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/illume` (adopted 2026-07). The former standalone repository is archived
> and links here.


illume takes source text and returns `(range, kind)` spans. It carries three passes:

- **djot structure** — headings, emphasis, strong, links, code, blockquotes, and the
  rest of the djot vocabulary, from [`jotdown`](https://crates.io/crates/jotdown)'s
  byte-offset event stream.
- **inner-language injection** — a pluggable `InjectionLexer` registry keyed by fence /
  language label, with a curated [`logos`](https://crates.io/crates/logos) pack (JSON,
  TOML, Rust, JS, Lua, CSS, HTML, and more) as the always-present floor. A host can
  register precise lexers over its own tokenizers as overrides.
- **prose entities** — URLs, `@mentions`, `#tags`, and emails, for enriching any text
  surface (an omnibar, a chat line, a note).

It also folds jotdown's nested events into a **container tree**, giving section folds, a
heading outline, and expand-to-enclosing structural selection.

illume is host- and toolkit-agnostic: it emits spans and knows nothing about colour or
rendering. Pair it with a palette (e.g. [`tinct`](https://crates.io/crates/tinct)) for
colour and a text surface for rendering. It builds for `wasm32-unknown-unknown` like any
pure-Rust crate — no C, no build apparatus.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
