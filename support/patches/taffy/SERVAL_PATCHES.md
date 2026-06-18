# serval's taffy fork — patch log

This is a vendored copy of `taffy 0.11.0-experimental-cache-fix.3` (the newest
published taffy, the only line carrying the experimental `float_layout`
feature). It is wired in via `[patch.crates-io] taffy = { path =
"support/patches/taffy" }` in the workspace `Cargo.toml`, and redirects only the
`=0.11.0-experimental-cache-fix.3` requirement (serval-layout + stylo_taffy);
the workspace's plain `taffy 0.10.1` is unaffected.

It exists because taffy's float / BFC / table layout is experimental and
incomplete, and serval pushes on exactly those paths as CSS conformance climbs.
This fork is the home for the layout fixes we accumulate, each upstreamed at our
own pace so the divergence from upstream stays small.

## How to keep it in sync

The vendored `src/` is upstream-pristine **except** for the files listed below.
When bumping taffy, re-vendor the new release and re-apply each patch (the
`.patch` files here are `git apply -p1`-able against an upstream checkout).
`diff -rq` against the pristine registry source must show only the listed files.

## Patches

### 0001 — `find_content_slot` width-fit (`0001-find_content_slot-width-fit.patch`)

**Files:** `src/compute/float.rs`, `src/compute/block.rs`
**Upstream status:** present on taffy `main` too; PR drafted (see
`UPSTREAM_PR.md`).

`FloatContext::find_content_slot` chose the first vertical band below `min_y`
without regard to whether the placed content is wide enough to fit there. A
full-width float makes that band zero-width (insets consume the whole
container), so a fixed-width BFC child was placed at the float's right edge and
overflowed, instead of dropping below the float to where it fits.

The fix threads the content's outer width through as `min_width`: the chosen
band must be at least that wide, otherwise the slot clears below all floats
(full container width). `min_width == 0` (auto-width / shrink-to-fit content,
which reflows to whatever the band offers) preserves the prior first-band
behaviour, so only fixed-width BFC children change. The block-layout caller
passes `item.size.width + non-auto x-margins` for fixed-width items and `0.0`
for auto-width.

Reftest moved: `css/CSS2/floats/floats-wrap-bfc-008` (fixed-width BFC clearing a
full-width float) now matches its reference.

### 0002 — float exclusion-band accessor (`0002-exclusion-bands.patch`)

**Files:** `src/compute/float.rs`, `src/compute/block.rs`, `src/compute/mod.rs`
**Upstream status:** serval-only so far (the inline IFC seam it feeds is
serval's parley-measured leaf, which upstream taffy does not model). Additive —
no existing taffy behaviour changes.

Inline text wrapping *around* a float needs each line box to know the width the
floats leave at its own y. taffy places floats (the `float_layout` feature) but
exposes only `find_content_slot` (one slot for one block child); it has no way
to hand a paragraph's line breaker the full set of exclusion bands.

This adds a read-only accessor and a small value type, leaving placement
untouched:

- `float.rs`: `InlineFloatBand { y_start, y_end, left, right }` and
  `FloatContext::exclusion_bands(min_y) -> Vec<(Range<f32>, [f32; 2])>` — a thin
  filter over the existing `segments` walk (segments at/below `min_y` that
  impose an inset on either side), in BFC-root space.
- `block.rs`: `BlockContext::inline_exclusion_bands(min_y) -> Vec<InlineFloatBand>`
  — the same coordinate handling as `find_content_slot` (subtract `y_offset` for
  block-local y; `max` each segment inset with the content-box inset and re-base
  to the content-box edge), but returning every band rather than one slot.
- `mod.rs`: re-export `InlineFloatBand`.

Consumed in serval-layout: the box tree snapshots these bands per inline-context
leaf into `TextMeasureCtx`, and the parley measure drives `Layout::break_lines()`
with per-line `set_line_x` / `set_line_max_advance` so lines wrap to a float's
side and reclaim the column below it (the float-wrap first cut;
`docs/2026-06-18_float_wrap_spike.md`). Known limit: only x-axis content-box
insets are tracked, so a top padding/border on the leaf is not yet reflected in
the band's `y` (fine for the common no-top-padding case).
