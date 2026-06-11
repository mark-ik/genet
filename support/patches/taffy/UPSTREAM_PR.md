# Upstream PR draft — taffy

Target: `DioxusLabs/taffy`, branch `main`. The bug reproduces on `main`
unchanged (`find_content_slot` there still takes no width and selects the first
vertically eligible band). Patch: `0001-find_content_slot-width-fit.patch` (line
offsets differ from `main`; the change is identical in substance).

---

## Title

block/float: make `find_content_slot` respect the content's width (BFC must
clear a float it can't fit beside)

## Summary

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

### Reproduction

A 100px container, a 100px-wide left float of height 50, and a 50px-wide BFC
child (`overflow: hidden`). Expected: the child clears to `y = 50` (below the
float) where 50px fits. Actual: it's placed at `(100, 0)`, overflowing right.
WPT: `css/CSS2/floats/floats-wrap-bfc-008.html`.

## Fix

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

## Follow-up (not in this PR)

Auto-width (shrink-to-fit) BFC children that should clear a float still pass
`0.0`. Doing them right needs the child's min-content width at the call site (an
intrinsic-size pass), so the "fits beside vs drops below" test uses min-content
rather than the resolved width. Left out here to keep the change minimal and
regression-free; happy to follow up if wanted.
