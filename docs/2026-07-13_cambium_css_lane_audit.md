# Cambium CSS lane audit

**Date:** 2026-07-13
**Status:** E0b lane choice and clean-room seed database landed against Cambium
`1f2e38d99ad4` and the Genet engine tree currently housed in this repository.

This is the second receipt for the native CSS engine. The first audit found the
full current engine path consumes 126 longhands. This audit chooses the first
bounded lane and records the smaller property contract it actually needs.

## Decision

The first lane is **Cambium structural UI over Genet DOM**.

Cambium is the consumer and Genet owns style, layout, paint, and engine
selection. The native CSS engine therefore belongs on the Genet side of the
boundary. It must not become a Cambium dependency that engine crates import.

This choice replaces the old `xilem_serval`/host-chrome wording. The extracted
toolkit is Cambium. The engine product is Genet; current `serval-*` packages,
Rust types, repository paths, and the `serval-layout` crate are compatibility
identifiers until their source rename lands.

The lane is deliberately narrower than a complete host theme. It covers:

- CSS emitted by Cambium and Sprigging production source;
- Genet's baseline UA stylesheet, which every styled DOM currently receives;
- the selectors and values needed to render those declarations.

It excludes application-owned theme sheets because Cambium's proposed component
catalog has not yet been implemented in the extracted workspace. That corpus
must be added before this can be called the production host-chrome contract.

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
**14 longhands**. Four overlap, producing a **22-longhand union**.

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
| **Union** | **22** | `color`, `display`, `font-size`, `font-style`, `font-weight`, `height`, `left`, `list-style-type`, `margin-bottom`, `margin-left`, `margin-right`, `margin-top`, `overflow-x`, `overflow-y`, `padding-left`, `position`, `text-decoration-line`, `text-wrap-mode`, `top`, `white-space-collapse`, `width`, `z-index` |

Production evidence:

- Cambium `crates/cambium/src/{arrangement,grid,highlight,overlay,select,slider,styled_field,tags}.rs`;
- Sprigging `crates/sprigging/src/arrange.rs`;
- current Genet path `components/serval-layout/ua_defaults.rs`.

The clean-room database is
[`second-css-engine/properties.toml`](./second-css-engine/properties.toml). Each
row records the CSS name, inheritance, initial value, grammar, seed values,
animation class, and an official specification source. The `seed_values` field
describes the values the audited corpus needs first; `grammar` records the
normative property shape it grows toward.

## Boundary consequence

This is not a 22-accessor swap inside the current `serval-layout` crate. That
crate still has the 126-longhand Stylo contract and broad Stylo lifecycle types.
The first integration must provide a separate concrete style/layout path for
Cambium documents behind Genet's document-facing runtime boundary. Computed
values stay concrete inside each path.

The engine selector belongs in Genet's document/session construction. A Cargo
feature may omit one engine for a constrained build, but the default product
needs both engines available so fullweb can keep Stylo while Cambium documents
select the native engine.

## Remaining E0 work

The lane choice and seed database are landed. Before E1 code generation starts:

1. create the Cambium component-catalog/theme fixture;
2. merge its application-owned property corpus into the database;
3. add a fixture that fails when a Cambium-owned declaration is absent from the
   database;
4. settle the native engine's crate name and move the database into that crate.

The stop rule is simple: do not call the 22-property seed the production chrome
contract until the theme fixture is present and audited.

## Done condition

This audit is complete when the checked-in 22 names agree with Cambium and
Sprigging's production-generated declarations plus Genet's baseline UA sheet,
and `properties.toml` parses as TOML with exactly 22 unique property rows. That
condition holds at the sources named above.
