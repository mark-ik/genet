# Content Inspection — genet's devtools substrate (scope)

**Date**: 2026-06-13
**Status**: Scoping + slice 1. The drive half (synthetic input) shipped as
`pelt-desktop`'s `TileShell` (`f68b3845549`); this is the observe/inspect half.

## The gap that motivates it

mere's shipped Inspector (`meerkat/src/inspector.rs`, the diagnostics plan's D8;
the content-devtools pane in
[`mere/design_docs/.../2026-06-06_peripheral_panes_architecture.md`](../../mere/design_docs/mere_docs/technical_architecture/2026-06-06_peripheral_panes_architecture.md))
renders rich content rows — outgoing links, parse diagnostics, source kind, a
block-structure summary — **for the non-genet `EngineDocument` path**. For
genet-rendered HTML it prints *"HTML rendered through Genet; no EngineDocument
diagnostics"*. So genet, the engine, renders a page but hands its own Inspector
nothing to inspect: **genet-rendered content is second-class in mere's content
devtools.** Closing that is the point.

## The capability

A genet-level **content-introspection surface**: `DOM -> ContentReport`
(title, a structural outline of role + name, outgoing links, headings; later
scripts, metadata, source, then headers / cookies / trackers from the netfetch
layer). genet already renders the page into a `ScriptedDom`/`StaticDocument` and
already emits the accesskit a11y tree (`genet-render/src/a11y.rs`); this packages
the *content model* the Inspector wants out of the same DOM.

## One substrate, three readers

The content report is simultaneously:
- the **Inspector's read model** (mere's pane, and pelt's `inspect tile`),
- the **test oracle** — assert against the semantic report, not pixels (survives
  a theme change or a 1px nudge, doubles as a structure-regression guard), and
- the twin of the **accesskit a11y tree** (`a11y.rs`, the OS-accessibility
  surfacing of the same roles).

Drive (synthetic input, done) + observe (this) is the browser-grade convergence:
keyboard-operable = WCAG-operable = headlessly-pokeable = legibly-addressable, one
property the graphshell both-input-modes directive already names.

## produce / prove / consume

Same shape as render + present. **genet produces** the introspection surface;
**pelt proves** it mere-free (`inspect tile` is the reference content-devtools, at
1/20th the size); **meerkat's Inspector consumes** it (and the "no diagnostics for
genet HTML" line goes away). The surface is engine-level, not a pelt feature.

## Slices

1. **Structure slice (this pass)** — `content_report(dom)` = title + outline +
   links + headings, in genet-render; re-point pelt's driven tests off the tile
   tree and onto the report; a `TileShell::inspect_tile(id) -> ContentReport`
   stub. The oracle + the a11y/structure guard + the first inspector panel, one
   function.
2. **Scripts + metadata** — `<script>` inventory (inline + src; ties to the V4
   runtime), `<meta>`/`<title>`/canonical.
3. **Network slice** — headers, cookies, trackers, off the returning netfetch
   layer.
4. **The rendered pane** — a xilem-serval devtools view over the report (pelt's
   `inspect tile`, the mere-free reference), which mere's Inspector mirrors.

## Cross-references

- The Inspector pane + roster bridge: the mere peripheral-panes doc (above).
- The accesskit twin: `genet-render/src/a11y.rs`.
- The drive half: `pelt-desktop/tile_shell.rs`; the pelt plan
  ([`2026-06-12_pelt_development_plan.md`](archive/2026-06-12_pelt_development_plan.md), archived).
