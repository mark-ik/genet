# Genet consumed CSS property audit (current Serval paths)

**Date:** 2026-07-13  
**Status:** landed against Serval `6b955ff96ed` and Stylo `eec60c2464`

Genet is the engine product formerly called Serval. Current repository, crate,
and source paths retain `serval` compatibility names during the migration.

This is the shared first receipt for the Stylo pruning track and the second
CSS engine plan. It answers one narrow question: which CSS longhands does the
current Serval layout, paint, CSSOM, and animation path actually consume?

## Result

The current full Serval contract is **126 longhands across 16 Stylo style
structs**. The earlier estimate of 33 longhand accessors is not the engine
seam. It describes only a convenient subset of accessor-shaped reads.

| Consumer surface | Longhands | Evidence |
| --- | ---: | --- |
| Taffy layout adapter | 59 | `support/patches/stylo_taffy/src/{wrapper,convert}.rs`, with default block/flex/grid features plus `floats` |
| Direct Serval render/text/box reads | 73 | `components/serval-layout`, especially `paint_emit.rs`, `box_tree.rs`, and `construct/` |
| `getComputedStyle` exposure | 30 | `components/serval-layout/computed_query.rs` |
| Animation and transition controls | 13 | Serval's Stylo animation path and `style/servo/animation.rs` |
| **Union** | **126** | Overlaps collapsed in the table below |

The 30-property CSSOM table is the closest live number to the old 33-property
claim. The active Taffy adapter already exceeds it before paint, text, list,
pseudo-content, CSSOM, or animation behavior enters the picture.

## Exact overlap table

Legend:

- `L`: Taffy layout adapter
- `R`: direct Serval render, text, box, or construction read
- `C`: `getComputedStyle`
- `A`: animation or transition control consumed by the incumbent engine

| Consumers | Count | Longhands |
| --- | ---: | --- |
| `R` | 43 | `background-attachment`, `background-clip`, `background-image`, `background-origin`, `background-position-x`, `background-position-y`, `background-repeat`, `background-size`, `border-bottom-color`, `border-bottom-left-radius`, `border-bottom-right-radius`, `border-image-outset`, `border-image-repeat`, `border-image-slice`, `border-image-source`, `border-image-width`, `border-left-color`, `border-right-color`, `border-top-color`, `border-top-left-radius`, `border-top-right-radius`, `box-shadow`, `clip-path`, `contain`, `content`, `filter`, `image-rendering`, `letter-spacing`, `list-style-position`, `list-style-type`, `mix-blend-mode`, `object-fit`, `order`, `perspective`, `pointer-events`, `text-decoration-color`, `text-decoration-line`, `text-overflow`, `text-wrap-mode`, `translate`, `will-change`, `word-spacing`, `z-index` |
| `L` | 30 | `align-content`, `align-items`, `align-self`, `aspect-ratio`, `box-sizing`, `column-gap`, `direction`, `flex-basis`, `flex-direction`, `flex-grow`, `flex-shrink`, `flex-wrap`, `grid-auto-columns`, `grid-auto-flow`, `grid-auto-rows`, `grid-column-end`, `grid-column-start`, `grid-row-end`, `grid-row-start`, `grid-template-areas`, `grid-template-columns`, `grid-template-rows`, `justify-content`, `justify-items`, `justify-self`, `max-height`, `max-width`, `min-height`, `min-width`, `row-gap` |
| `A` | 13 | `animation-delay`, `animation-direction`, `animation-duration`, `animation-fill-mode`, `animation-iteration-count`, `animation-name`, `animation-play-state`, `animation-timing-function`, `transition-behavior`, `transition-delay`, `transition-duration`, `transition-property`, `transition-timing-function` |
| `L R` | 10 | `border-bottom-style`, `border-bottom-width`, `border-left-style`, `border-left-width`, `border-right-style`, `border-right-width`, `border-top-style`, `border-top-width`, `clear`, `float` |
| `L R C` | 10 | `bottom`, `display`, `height`, `left`, `overflow-x`, `overflow-y`, `position`, `right`, `top`, `width` |
| `R C` | 10 | `background-color`, `color`, `font-family`, `font-size`, `font-style`, `font-weight`, `line-height`, `opacity`, `transform`, `white-space-collapse` |
| `L C` | 9 | `margin-bottom`, `margin-left`, `margin-right`, `margin-top`, `padding-bottom`, `padding-left`, `padding-right`, `padding-top`, `text-align` |
| `C` | 1 | `visibility` |

## Incumbent style-struct distribution

This mapping describes the current Stylo storage organization. It is useful
for fork pruning and for locating direct reads. It is not a clean-room design
input for the new engine.

| Stylo struct | Count |
| --- | ---: |
| Position | 38 |
| Border | 21 |
| UI | 13 |
| Box | 11 |
| Background | 9 |
| InheritedText | 6 |
| Font | 5 |
| Effects | 4 |
| Margin | 4 |
| Padding | 4 |
| InheritedBox | 3 |
| Text | 3 |
| List | 2 |
| Counters | 1 |
| InheritedUI | 1 |
| SVG | 1 |

All 126 names resolve to longhands enabled for Servo in the realigned v0.19
fork. Seventeen are inherited through the incumbent struct organization; 109
are non-inherited.

## What this means for the second engine

The proposed 33-accessor-compatible `ComputedValues` is insufficient for a
feature swap of the current `serval-layout` crate. The live crate also relies
on:

- direct fields on 16 style structs;
- Stylo value and generic types in paint, gradients, borders, shapes, text,
  grid, flex, and sizing;
- `Stylist`, rule-tree, declaration-block, restyle, selector-adapter, and
  animation APIs outside `ComputedValues`;
- 257 `style::` references spread across 24 `serval-layout` source files.

A Cargo feature can select one engine for a build. It cannot provide the
per-document engine choice promised by the plan. Runtime selection needs both
engines in the binary and a shared document-facing style contract, or separate
layout implementations behind a higher-level document multiplexer.

There are therefore two honest scopes:

1. **Full `serval-layout` swap parity:** the new engine starts with this
   126-longhand contract and replaces the wider Stylo lifecycle APIs too.
2. **Lane-first engine:** chrome, smolweb, or cards get a smaller audited
   property set and a separate layout/style boundary. That subset must be
   measured from the chosen lane's stylesheets and reftest corpus. It cannot be
   inferred from the old 33-accessor count.

The lane-first scope remains the plausible one. It requires choosing the first
lane before writing the property database.

## Clean-room boundary

This audit records public CSS names and live consumer call sites. It does not
copy Stylo value implementations, initial-value expressions, parsers, or
generated property metadata. The new engine's `properties.toml` must source
inheritance, initial values, grammars, and animation classes independently
from specifications and clean-room references.

That splits the old E0 receipt into two parts:

- **E0a, landed here:** current-consumer census and seam correction.
- **E0b, landed separately:** the Cambium structural lane audit derives a
  22-longhand seed and authors the clean-room property database from
  specifications. Its component-catalog theme expansion remains a pre-E1 gate.

## Fork pruning use

These 126 longhands are the hard keep-set for the current Serval product path.
They are not by themselves a safe deletion allowlist. Shorthands,
`transition-property`, CSS-wide keywords, keyframes, custom-property
substitution, and CSSOM parsing can retain dependencies on longhands that
layout never reads. Any fork deletion proposal must subtract this keep-set
from Servo-enabled longhands, then pass the workspace, layout suites, and WPT
walls after each generated-property batch.

## Done condition

The audit is complete when the checked-in count and lists agree with:

- active `stylo_taffy` features in `components/serval-layout/Cargo.toml`;
- direct computed-style reads under `components/serval-layout`;
- `computed_query.rs`'s supported names;
- the animation and transition controls read by the Servo animation engine.

That condition holds at the hashes named above.
