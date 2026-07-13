# genet's taffy fork — patch log

This is a vendored copy of `taffy 0.12.1` — re-vendored 2026-07-12 from the
prior `0.11.0-experimental-cache-fix.3` (`float_layout` graduated from
experimental to stable in 0.12, ending the need to ride the experimental
line at all; see `docs/2026-07-12_ring3_fork_rename_publish_plan.md`, T0). It
is wired in via `[patch.crates-io] taffy = { path =
"support/patches/taffy" }` in the workspace `Cargo.toml`, and redirects only the
`=0.12.1` requirement (genet-layout + paint + genet-render + genet-wpt +
the vendored `stylo_taffy`); the workspace's plain `taffy 0.10.1` is
unaffected.

It exists because taffy's float / BFC / table layout is still incomplete in
places, and genet pushes on exactly those paths as CSS conformance climbs.
This fork is the home for the layout fixes we accumulate, each upstreamed at our
own pace so the divergence from upstream stays small — see `UPSTREAM_PR.md`
for the drafted PRs (all three patches below are PR-able; none has landed
upstream yet).

## How to keep it in sync

The vendored `src/` is upstream-pristine **except** for the files listed below.
When bumping taffy, re-vendor the new release and re-apply each patch (the
`.patch` files here are `git apply -p1`-able against an upstream checkout).
`diff -rq` against the pristine registry source must show only the listed files.

## Patches

### 0001 — `find_content_slot` width-fit (`0001-find_content_slot-width-fit.patch`)

**Files:** `src/compute/float.rs`, `src/compute/block.rs`
**Upstream status:** present on taffy `main` too (unfixed as of 0.12.1); PR
drafted (see `UPSTREAM_PR.md`, PR 1).

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
**Upstream status:** genet-only so far (the inline IFC seam it feeds is
genet's parley-measured leaf, which upstream taffy does not model). Additive —
no existing taffy behaviour changes. PR drafted (`UPSTREAM_PR.md`, PR 2).

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

Consumed in genet-layout: the box tree snapshots these bands per inline-context
leaf into `TextMeasureCtx`, and the parley measure drives `Layout::break_lines()`
with per-line `set_line_x` / `set_line_max_advance` so lines wrap to a float's
side and reclaim the column below it (the float-wrap first cut;
`docs/2026-06-18_float_wrap_spike.md`). Known limit: only x-axis content-box
insets are tracked, so a top padding/border on the leaf is not yet reflected in
the band's `y` (fine for the common no-top-padding case).

### 0003 — flex `order` support (`0003-flex-order.patch`)

**Files:** `src/style/flex.rs`, `src/compute/flexbox.rs`
**Upstream status:** genet-only. This taffy version does not model CSS `order`
at all — `FlexItem.order` is the document index (used only for paint/output
ordering), and the flex algorithm processes items in document order. PR
drafted (`UPSTREAM_PR.md`, PR 3).

CSS `order` lays flex items out (and paints them) in *order-modified document
order*: items sort by ascending `order`, ties broken by document order.

- `flex.rs`: add `FlexboxItemStyle::order() -> i32` (default 0). Adapters that
  don't override it keep document order, so existing behaviour is unchanged.
- `flexbox.rs`: `FlexItem` gains a `css_order: i32` field, populated from
  `child_style.order()` in `generate_anonymous_flex_items`; after collection the
  item vec is `sort_by_key(|i| i.css_order)` — a *stable* sort, so equal-`order`
  items (the common case, 0) keep document order. The pre-existing `order: u32`
  field is left as the document index for paint/output ordering, so paint order
  is unchanged (a deliberate first-cut limit: CSS `order` also re-orders
  painting, but genet paints in document order regardless, and flex items rarely
  overlap).

Consumed in genet-layout: `box_tree.rs`'s `CssStyle` flex-item wrapper overrides
`FlexboxItemStyle::order()` to read `get_position().order` off the cascade (the
same wrap-and-override pattern it already uses for grid placement; no
`stylo_taffy` patch needed). Verified by `flex_order_reorders_items` and
`flex_order_is_stable_and_handles_negative`.
