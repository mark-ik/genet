# Cambium CSS lane audit

**Date:** 2026-07-13
**Status:** E0b lane choice, themed catalog fixture, and original 40-property
clean-room database landed. The E1 handoff into Livery and the first E3
`genet-livery` integration slice have also landed. The native capability
ratchet now contains 87 properties, including geometry, corner radii,
visibility, alignment, spacing, box shadows, two-stop linear-gradient
backgrounds, bounded opacity keyframes, flexbox, and bounded grid placement. Source
hashes are recorded below.

Audited revisions: Cambium `a7c4603c` for the live catalog and Genet
`c00daa92308` for the database, snapshot, and executable guard. The local
Cambium extraction does not yet have a Git remote configured.

This is the second receipt for the native CSS engine. The first audit found the
full current engine path consumes 126 longhands. This audit chooses the first
bounded lane and records the smaller property contract it actually needs.

## Decision

The first lane is **Cambium structural UI over Genet DOM**.

Cambium is the consumer and Genet owns style, layout, paint, and engine
selection. The native CSS engine therefore belongs on the Genet side of the
boundary. It must not become a Cambium dependency that engine crates import.

This choice replaces the old `xilem_serval`/host-chrome wording. The extracted
toolkit is Cambium. The engine product is Genet; current `genet-*` packages,
Rust types, repository paths, and the `genet-layout` crate are compatibility
identifiers until their source rename lands.

The lane is deliberately narrower than a complete application theme. It covers:

- CSS emitted by Cambium and Sprigging production source;
- Genet's baseline UA stylesheet, which every styled DOM currently receives;
- the selectors and values needed to render those declarations;
- Cambium's first real component-catalog theme, covering its button, checkbox,
  toggle, radio, select, slider, and text-field compositions.

It does not claim to be Merecat's production theme. The fixture is the bounded
toolkit corpus that lets the engine grow before an application theme is ready.

## Why this lane

Cambium already emits real DOM `style` attributes and class-based color rules.
It exercises positioned overlays, arrangements, grid virtualization, external
textures, custom leaves, text fields, and native controls through Genet's
cascade and layout.

The other candidates do not give a cleaner first boundary:

- Engine-native Nematic lowers `EngineDocument` to `PaintList`; it does not use
  CSS. A later `cambium-nematic` DOM view is a Cambium consumer and can grow this
  lane, but it is not the engine-native smolweb path.
- Cards use Cambium and Sprigging, so they are an application corpus for this
  same lane rather than a distinct style engine boundary.
- Fullweb stays on Genet Stylo.

## Exact seed

Cambium and Sprigging emit **12 longhands**. Genet's baseline UA sheet adds
**14 longhands**. Four overlap, producing the original **22-longhand structural
seed**. The catalog theme exercises 30 longhands after expanding `border`,
`margin`, and `padding`; 12 were already in the structural seed, so the theme
adds 18 and produces a **40-longhand database**.

The two toolkit shorthands were removed in Cambium `1f2e38d99ad4`:

- `overflow: hidden` became explicit `overflow-x` and `overflow-y`;
- `text-decoration: underline` became `text-decoration-line: underline`.

That keeps the first database one row per emitted property. The UA sheet still
uses `margin` and `white-space` shorthands; its parser must expand those into
the longhands below.

| Source | Count | Longhands |
| --- | ---: | --- |
| Cambium/Sprigging production source | 12 | `color`, `display`, `font-style`, `height`, `left`, `overflow-x`, `overflow-y`, `position`, `text-decoration-line`, `top`, `width`, `z-index` |
| Genet baseline UA sheet | 14 | `display`, `font-size`, `font-style`, `font-weight`, `height`, `list-style-type`, `margin-bottom`, `margin-left`, `margin-right`, `margin-top`, `padding-left`, `text-wrap-mode`, `white-space-collapse`, `width` |
| Structural union | 22 | The two rows above, with four overlaps collapsed |
| Cambium catalog theme | 30 | Direct declarations plus `border`, `margin`, and `padding` expansion |
| **Final union** | **40** | Structural seed plus the 18 additions below |

The theme adds `background-color`, `font-family`, `line-height`,
`padding-bottom`, `padding-right`, `padding-top`, and all twelve physical
`border-*-{color,style,width}` longhands.

Production evidence:

- Cambium `crates/cambium/src/{arrangement,grid,highlight,overlay,select,slider,styled_field,tags}.rs`;
- Sprigging `crates/sprigging/src/arrange.rs`;
- current Genet path `components/genet-layout/ua_defaults.rs`;
- Cambium `crates/cambium/examples/{component_catalog.rs,component_catalog.css}`;
- Livery's audited snapshot
  `components/livery/tests/fixtures/cambium-component-catalog.css`.

The clean-room database is
[`components/livery/properties.toml`](../components/livery/properties.toml). Each
row records the CSS name, concrete value family, inheritance, initial value,
grammar, seed values, animation class, and an official specification source.
The `seed_values` field
describes the values the audited corpus needs first; `grammar` records the
normative property shape it grows toward.

## Boundary consequence

This is not a 42-accessor swap inside the current `genet-layout` crate. That
crate still has the 126-longhand Stylo contract and broad Stylo lifecycle types.
The first integration must provide a separate concrete style/layout path for
Cambium documents behind Genet's document-facing runtime boundary. Computed
values stay concrete inside each path.

The engine selector belongs in Genet's document/session construction. A Cargo
feature may omit one engine for a constrained build, but the default product
needs both engines available so fullweb can keep Stylo while Cambium documents
select the native engine.

The first integration slice now exists as `components/genet-livery`. It adapts
the shared `LayoutDom` surface directly, owns a concrete Livery style plane,
lays the audited box subset out through standalone Taffy, and emits box
backgrounds and physical borders through the neutral `PaintList` API. Text and
nested inline elements now share a Parley formatting context, and retained
`LiveryDocument` ownership keeps font resources and unchanged frames stable.
With the `genet-documents` `livery` feature, the path enters the session
registry as the explicitly pinned `genet.livery` static rung and lowers through
the shared netrender translator. Parley's positioned output now replaces the
placeholder block boxes for inline paint: wrapped spans emit one fragment per
line, and `inline-block` children take atomic space in the shared line.
Retained sessions now resolve atomic box sizes in a preliminary Taffy pass,
then collapse each consecutive inline group into one Parley-measured Taffy
leaf. The shared line layout drives both paint geometry and parent block flow,
including mixed text, styled spans, and atomic boxes. `genet.web` remains the
default. Inline padding and borders now consume horizontal line advance, paint
vertical overflow without altering line height, and use slice edges across
wrapped fragments. Each inline group also retains Parley's visual item order
for bidi paint while prepainting its inline decorations. Block containers now
wrap descendants in axis-aware padding-box clips for non-visible overflow, with
nested clips balanced through the neutral paint list. Within each numeric or
document stacking context, positioned block and inline-level elements with
numeric `z-index` now paint in stable negative, normal-flow, then nonnegative
order, with reordered subtrees kept atomic. Numeric roots also flatten through
intervening ancestors into the nearest context, carrying ancestor overflow
clips with them. Ownership retained on shaped inline commands lets positioned
spans and atomic inline-blocks reorder without losing shared line geometry or
bidi visual order. Opacity below one now creates an atomic level-zero context
and a neutral compositing layer around the subtree. Transform context triggers
have also landed for transformable block and inline-block boxes. The
bounded transform list supports 2D translate, scale, and rotate functions
around the default center origin. The retained Livery session now routes
viewport scroll, pointer-events hit testing, links, fragment navigation, and
focus state. A host-driven opacity clock, bounded CSS transition metadata for
opacity, background-color, and color, and a native reftest-style paint pair are
covered by package receipts. Nested scroll containers route wheel deltas into retained offsets and
chain to the viewport at their boundary. Raster `data:` background URLs reach
the neutral image side-table, and host-resolved local image bytes now use the
same side-table seam. Bounded intrinsic tiling and position/repeat modes now
paint through that seam. The bounded `all` and explicit two- and three-property
transition paths now sample opacity, background-color, and color together. Replaced
`<img>` elements now use intrinsic data/local dimensions and preserve their
aspect ratio under a bounded CSS width or height. Host-supplied remote-looking
URLs use the same image seam. The Livery session now asks the host fetcher for
remote image bytes and paints them through that seam; full WPT reftest parity
remains open. Bounded
opacity-only `@keyframes` declarations
with linear, ease, ease-in, ease-out, and ease-in-out timing functions now run
on the retained clock. The WPT command surface now accepts
`--renderer livery` for a bounded producer route covering inline and local linked
stylesheets, local image URLs, and raster `data:` backgrounds. Stylo remains the default, and
the route is a comparison seam rather than a full-corpus parity receipt.

The 2026-07-15 gate pass adds a host-supplied remote-looking URL receipt, a
session-level remote image fetch receipt, and bounded explicit two- and
three-property transition receipts, including text color. The producer helper
tests and `css/CSS2/box/ltr-basic.xht` pass on both routes. Focused
`background-image-001.html` and `background-color-clip.html` reftests also pass
on both routes. A first
`css/css-backgrounds` probe reports 7 passes, 582 failures, 360 skips, and no
runner errors. That is a capability map rather than a parity receipt: the
failures cluster around background features outside the bounded lane and
crash-path coverage. Remote URL policy and caching remain host-owned.

## E0 closeout

The 2026-07-15 capability receipt grows the original catalog plus the first
two ratchet rows, bounded transition controls, and opacity keyframe controls to
87 properties. Geometry,
flexbox, and bounded grid values
are lowered into Taffy; corner radii, color fills, and two-stop gradients reach
neutral paint commands; Parley consumes text alignment and spacing; and hidden
boxes retain layout space while suppressing paint. This is an E3 capability
ratchet, not a change to the original catalog census.

The lane choice, themed fixture, expanded database, and executable coverage
guard are landed. `components/livery/tests/catalog_contract.rs` checks the
catalog declarations against Livery's generated property and shorthand tables
and fails if a required longhand is absent. This completes the audit's E1
handoff: the named engine crate owns the database, fixture, generator, typed
computed values, and guard.

The original 40-property set is the catalog contract, not a promise about every
future Merecat theme declaration. Capability-driven rows may join it with their
own executable paint proof; `opacity` and `transform` are the first two ratchet
rows.

## Done condition

This audit is complete when the checked-in names cover Cambium and Sprigging's
production-generated declarations, Genet's baseline UA sheet, and the catalog
theme after shorthand expansion; `properties.toml` must preserve those 40 rows
plus explicitly tested capability-ratchet rows, including opacity, transform,
transition, and keyframe controls. The executable
guard and independent TOML check satisfy that condition.
