# nematic

`nematic` is the portable smolweb engine family for Genet and its hosts. It
lowers Gemini, Gopher, Spartan, Finger, plain text, Markdown, RSS/Atom, and
the knot note format into Inker's `EngineDocument` model for stored,
authored, and worker-shippable content. Cambium-native projections over the
protocol ASTs live separately in `cambium-nematic`.

> **Home:** [`mark-ik/genet`](https://github.com/mark-ik/genet), at
> `components/nematic` (adopted 2026-07). The former standalone repository is archived
> and links here.


For fullweb rendering (CSS, JS, embedded media), mere routes through Genet
(the Servo/wgpu fork) or a system webview. **Nematic does not own an HTML
reader-mode lane** — that's Genet's future "three-head Hekate" mode
(smolweb extract / middlenet / fullweb negotiator for the same HTML input).
Nematic stays for protocols whose grammar the engine can fully parse
natively.

## Naming

*Nematic* is borrowed from liquid-crystal physics: a nematic phase has
*orientational* order without *positional* order; rod-shaped molecules all
point the same way but otherwise flow freely. Light passes through aligned
nematic crystals coherently, and that's the basis of LCDs.

If the web is a lenticular soup of pixels, then nematic is the engine that
tries to align the molecules and let the light through.

## Engines

Twelve concrete `inker::Engine` implementations, each spec-faithful to its
source format. Use [`engines()`](src/lib.rs) to register all twelve in one
call.

| Engine | ID | Module | Notes |
| --- | --- | --- | --- |
| Markdown | `nematic.markdown` | [`markdown`](src/markdown.rs) | CommonMark via [`pulldown-cmark`] |
| Gemtext | `nematic.gemtext` | [`gemtext`](src/gemtext.rs) | Gemini's `text/gemini` line-oriented format |
| Gopher | `nematic.gopher` | [`gopher`](src/gopher.rs) | RFC 1436 menu parser; synthesised `gopher://` URLs per RFC 4266 |
| Feed | `nematic.feed` | [`feed`](src/feed.rs) | RSS 2.0 + Atom 1.0 via `quick-xml`; emits `FeedHeader` + `FeedEntry` semantic blocks |
| Text | `nematic.text` | [`text`](src/text.rs) | Plain text with paragraph splitting |
| File | `nematic.file` | [`file`](src/file.rs) | Extension-based dispatch for `file://` content (`.md`/`.gmi`/`.gophermap`/`.xml`/`.knot`/…) |
| Finger | `nematic.finger` | [`finger`](src/finger.rs) | RFC 1288 finger responses; tags `text/x-finger` |
| Knot | `nematic.knot` | [`knot`](src/knot.rs) | Mere's native note / clip format (frontmatter + polyglot markdown) |
| Scroll | `nematic.scroll` | [`scroll`](src/scroll.rs) | scroll.mozz.us body engine; delegates to gemtext or markdown by content-type |
| Misfin | `nematic.misfin` | [`misfin`](src/misfin.rs) | misfin.org gemini-style mail body |
| Nex | `nematic.nex` | [`nex`](src/nex.rs) | Nex directory listings + plain text content |
| Guppy | `nematic.guppy` | [`guppy`](src/guppy.rs) | UDP-smolweb body (gemtext shape) |

All engines populate `EngineDocument.provenance` with their own engine ID
and the request address; trust state defaults to `Unknown` (the host
overrides after transport verification).

## Knot: the native note / clip format

**Knot** (`nematic.knot`) is Mere's polyglot note format and the load-bearing
output of the clip workflow. A knot body is CommonMark with fenced code
blocks whose language tag dispatches to a real engine:

```text
---
title: Mixed Clip
source: https://blog.test/article
captured: 2026-05-08T14:23:00Z
trust: tofu
note_kind: clip
tags: [research, semantics]
---

User prose with [[wikilinks]] and #hashtags.

` ` `gemtext
=> gemini://capsule.test/ a capsule
* a bullet
` ` `

` ` `feed-entry
title: Linked article
url: https://blog.test/post
date: 2026-05-08
` ` `

` ` `gopher
0README<TAB>/readme.txt<TAB>example.org<TAB>70
` ` `
```

- **Frontmatter** (YAML subset) populates `provenance` (`source`,
  `captured`, `source_label`), `trust` state, and emits `note_kind` /
  `tags` as `MetadataRow` blocks.
- **Fenced protocol blocks** (`gemtext`, `gopher`, `nex`, `feed-entry`,
  `feed-header`, `metadata-row`, `badge`) are expanded into real semantic
  blocks by `nematic::knot::expand`. Unknown languages (e.g. `python`,
  `rust`) pass through as code blocks unchanged.
- **Wikilinks** `[[name]]` rewrite to `mere://node/<slug>` (slug is
  lowercased, whitespace → `-`); display text preserves the original.
- **Hashtags** `#tag` at word boundaries are extracted from paragraph text
  and emitted as `Badge` sibling blocks (so search / intelligence layers
  see them as semantic markers).
- **`build_clip_knot(blocks, provenance, trust, note_kind)`** assembles a
  ready-to-save `.knot` string from selected blocks plus the source's
  provenance. The host's clip gesture wires up to this once the clip UI
  lands.
- **`build_clip_knot_with_block_provenance(...)`** is the multi-source
  variant: takes an additional `inker::BlockProvenanceMap` sidecar and
  emits a `block_sources: ["<index>|<uri>[|<anchor>]", ...]` frontmatter
  list for blocks whose source differs from the document. Use this when
  composing a clip from heterogeneous sources (federated feed merge,
  citation overlay, multi-tab clip). Round-trip restoration through
  `KnotEngine` is gated on a concrete consumer; the producer side
  documents the shape so downstream readers can parse it directly.
- **Round-trip**: `EngineDocument::to_knot()` (in `inker`) re-emits semantic
  blocks as fenced code blocks with their language tag, so a parsed knot
  serialises back into an equivalent knot.

See [`design_docs/nematic_docs/implementation_strategy/2026-05-08_polyglot_knot_design.md`](../../design_docs/nematic_docs/implementation_strategy/2026-05-08_polyglot_knot_design.md)
for the full design.

[`pulldown-cmark`]: https://crates.io/crates/pulldown-cmark

## How it relates to other workspace crates

nematic is the engine that [`inker`](https://crates.io/crates/inker)
dispatches to for smolweb URI schemes; rendered output is presented through
[`verso-core`](https://crates.io/crates/verso-core)'s surface contracts.

```text
   inker.routing
      │ EngineRouteDecision
      │ engine_id ∈ { nematic.smolweb, nematic.file }
      ▼
   nematic
      │ rendered content
      ▼
   verso-core (CompositedTexture surface)
```

- [`inker`](https://crates.io/crates/inker) — references nematic by engine
  ID. The default policy routes `gemini`, `gopher`, `finger`, `spartan` →
  `nematic.smolweb`, and `file` → `nematic.file`.
- [`verso-core`](https://crates.io/crates/verso-core) — nematic's output is
  presented as a `CompositedTexture` surface; verso-core owns the surface
  lifecycle.
- [`mere`](https://crates.io/crates/mere) — composes nematic into the
  product.

## Status

Pre-1.0. Markdown lane shipped; smolweb (gemini/gopher), file, and feed
lanes pending. Implementation is in progress within the
[mere workspace](https://github.com/mark-ik/mere).

## Fun Fact

My first idea for the crate's name was middlenet, intended to encapsulate
the smolweb and well-structured web content. This notion of a browser
that could manage whatever protocol it was offered calls to mind a quote
from the game Elden Ring:

"Heresy is not native to the world; it is but a contrivance.
All things can be conjoined."

Accordingly, another possible name was "miriel," and a fourth, "turtlepope."
All protocols *can* be conjoined?

## License

MPL-2.0.
