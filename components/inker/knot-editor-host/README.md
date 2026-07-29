# knot-editor-host

Host-side reuse-lexers for the knot editor: precise inner-language injection
over parsers the workspace already ships, layered as overrides on the
portable [illume](https://crates.io/crates/illume) pack. The editor model
ties source text to the rendered preview through the same engine path the
rest of the family uses (inker's `EngineDocument` + nematic's
`DjotKnotEngine`).

> **Home:** [`merely-made/genet`](https://github.com/merely-made/genet), at
> `components/inker/knot-editor-host` (adopted from mere 2026-07-10 with the
> engine-management family). Markdown via pulldown-cmark today;
> CSS/HTML/JS/Turtle to follow.
