# Upstream PR drafts — taffy

Target: `DioxusLabs/taffy`, branch `main`, against the **released 0.12.1**
shape (`float_layout` graduated from experimental to stable in this release —
genet's fork re-vendored onto it 2026-07-12, ending the need to ride the
`-experimental-cache-fix` line; see
`docs/2026-07-12_ring3_fork_rename_publish_plan.md`, T0). All three patches
below are verified: applied against 0.12.1's actual source, compiled, and run
through genet-layout's full suite (320 tests) + the html-to-pixels paint
corpus + all nine WPT baselines with zero regressions. `main` may have moved
further since 0.12.1's cut; line offsets in the `.patch` files will need
rebasing, but each fix's substance is unchanged.

---

## PR 1 — `find_content_slot` width-fit

**Title:** block/float: make `find_content_slot` respect the content's width
(BFC must clear a float it can't fit beside)

### PR 1 summary

A block box that establishes a new formatting context (e.g. `overflow: hidden`)
must not let its border box overlap a float in the same BFC (CSS 2.1 §9.5,
"flow around floats"). When it doesn't fit in the space beside the float it
drops below.

`FloatContext::find_content_slot` picks the first vertical band whose `y.end >
min_y` and returns it regardless of the band's available width:

```rust
.position(|segment| segment.y.end > min_y)
```

So a **full-width float** produces a zero-width band (its insets consume the
whole container), and a fixed-width BFC child is handed a slot at the float's
right edge. The caller in `compute/block.rs` keeps the child's fixed width, so
it renders past the container edge instead of dropping below the float.

### PR 1 reproduction

A 100px container, a 100px-wide left float of height 50, and a 50px-wide BFC
child (`overflow: hidden`). Expected: the child clears to `y = 50` (below the
float) where 50px fits. Actual: it's placed at `(100, 0)`, overflowing right.
WPT: `css/CSS2/floats/floats-wrap-bfc-008.html`.

### PR 1 fix

Thread the content's minimum (outer) width into `find_content_slot`:

- Skip bands narrower than `min_width` during the scan.
- If no band from `min_y` down is wide enough, clear below all floats, where the
  full container width is available (previously this `None` arm returned `min_y`
  at full width, overlapping the floats — only reachable now via the width
  miss).
- `min_width == 0` reproduces the old first-band behaviour, so auto-width /
  shrink-to-fit content (which reflows to whatever the band offers) is
  unchanged.

The block-layout caller passes the in-flow BFC child's resolved outer width
(`item.size.width + non-auto x-margins`) for fixed-width items, and `0.0` for
auto-width items.

### PR 1 follow-up (not in this PR)

Auto-width (shrink-to-fit) BFC children that should clear a float still pass
`0.0`. Doing them right needs the child's min-content width at the call site (an
intrinsic-size pass), so the "fits beside vs drops below" test uses min-content
rather than the resolved width. Left out here to keep the change minimal and
regression-free; happy to follow up if wanted.

---

## PR 2 — float exclusion-band accessor (additive, no behavior change)

Target: `DioxusLabs/taffy`, branch `main`. Patch:
`0002-exclusion-bands.patch`.

### PR 2 summary

`float_layout`'s `FloatContext` places floats and exposes
`find_content_slot` (one slot for one block child), but has no way to hand a
paragraph's *inline* line breaker the full set of horizontal exclusions a
float imposes over a y-range — needed to wrap text around a float and reclaim
the full line width below it (CSS 2.1 §9.5's other half: floats affect
in-flow *inline* content, not just BFC block children).

### PR 2 change

Purely additive, no existing behavior touched:

- `FloatContext::exclusion_bands(min_y) -> Vec<(Range<f32>, [f32; 2])>`: a
  read-only snapshot of the active float segments at/below `min_y`, each
  paired with its `[left, right]` insets (absolute, BFC-root coordinates).
  Segments imposing no inset on either side are omitted.
- `BlockContext::inline_exclusion_bands(min_y) -> Vec<InlineFloatBand>`: the
  same, converted to the **consuming block's content-box-local** coordinate
  space (mirrors `find_content_slot`'s existing `y_offset`/inset handling), as
  a new public `InlineFloatBand { y_start, y_end, left, right }` type.

### PR 2 consumer

genet-layout's parley-measured inline-formatting-context leaf reads
`inline_exclusion_bands` to narrow each line's available width by the active
float insets at that line's y — the wrap-around-a-float behavior a plain
`find_content_slot` (one slot per block child) can't provide for inline
content. No taffy-side test exists yet for the accessor itself (it has no
opinion on layout, only reports state); happy to add one alongside the PR if
useful.

---

## PR 3 — flex `order` support

Target: `DioxusLabs/taffy`, branch `main`. Patch: `0003-flex-order.patch`.

### PR 3 summary

taffy does not model the CSS `order` property: `FlexItem.order` is the
*document* index (used only for paint/output ordering), and the flex
algorithm processes items in document order regardless of any `order` value a
`FlexboxItemStyle` implementation might report. CSS requires flex items to
lay out — and paint — in **order-modified document order**: ascending
`order`, ties broken by document order
(<https://www.w3.org/TR/css-flexbox-1/#order-property>).

### PR 3 change

- `FlexboxItemStyle::order() -> i32` (default `0`, `#[inline(always)]`):
  additive trait method, so every existing `FlexboxItemStyle` implementor is
  source-compatible without change (default keeps document order).
- `FlexItem` gains `css_order: i32`, populated from `child_style.order()` in
  `generate_anonymous_flex_items`.
- Immediately after generation, `flex_items.sort_by_key(|item| item.css_order)`
  — a **stable** sort, so the common all-`order:0` case is unaffected (ties
  keep document order) and only items with an explicit `order` move.

The pre-existing `order: u32` field (the document index) is untouched, so
paint/output ordering — which taffy's own consumers may rely on separately —
does not change; only the *layout* (and therefore visual) order does.

### PR 3 consumer

genet-layout's `CssStyle` flex-item wrapper overrides `order()` to read the
cascaded `order` property (`get_position().order`) off stylo's
`ComputedValues` — the same wrap-and-override pattern it already uses for
grid placement (`GridItemStyle::grid_row`/`grid_column`). Verified against a
four-item `order: 3, 1, -1, 0` reftest-shaped fixture.
