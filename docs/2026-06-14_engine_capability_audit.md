# Engine capability audit: hit-testing + browser-readiness, re-grounded

**Date:** 2026-06-14. **Method:** two parallel-agent sweeps verified against the
actual serval + mere code (file:line), prompted by the inline-link hit-testing
gap. The prior roadmap labels were steering by a stale 2026-06-12 scoping
snapshot; this corrects them.

## Landed this session

- **Inline-box hit-testing** (`serval-layout/inline_hit.rs`, commit
  `4a159c24074`). A `display:inline` element establishes no box, so the block
  hit-walk could only resolve its containing block; `construct` now records a
  byte-range -> source-element index per inline-formatting leaf, `BoxTree`
  retains it, and `hit_test` descends into the leaf's parley layout to resolve
  the element under the point. Standards-correct (CSSOM View; CSS2.2 §9.4.2):
  the set of per-line run rects, containment-tested, never a union box.
- **`pointer-events` hit-testing** (commit `e489382c88c`). A
  `pointer-events:none` box is not a hit target (falls through); the walk still
  descends, so a `pointer-events:auto` descendant stays hittable. The property
  inherits, so the per-box computed value already encodes the cascade (the
  CSS-UI non-blanket rule, no extra tree state).

## Corrected state (verified, not labelled)

**Done** (several were mislabelled partial/missing):

- Document / viewport scroll: `Viewport` object, root->viewport overflow
  propagation with `<body>` fallback + `overflow:hidden`-disables, scrollable-
  overflow range, `Fixed != Absolute` (fixed pinned in paint and hit),
  `%`-height chain, `vw`/`vh` + resize re-resolution. All tested.
- Inline-box hit-testing, `pointer-events` (this session).
- Single- and **multi-node** text selection geometry (`caret::range_rects`
  walks leaves in tree order, unions per-line rects; consumed by meerkat).
- **Focus model + Tab order**: DOM-order Tab/Shift+Tab cycling, click-to-focus
  on the nearest focusable, Enter/Space activation, handler-first overridable
  Tab. (Only scroll-into-view / tabindex / autofocus remain.)
- **CSS text-decoration** all three lines (overline reconstructed from the
  source run, since parley carries none).
- **A11y tree + OS adapters** (`serval-render/a11y.rs` -> AccessKit; meerkat
  wires Win/macOS/Linux). Block-level bounds present.
- Host-side form controls + IME preedit (xilem-serval). Affordances query.
  Tab-drag / divider-drag gestures (pelt).

**Genuine gaps**, ranked by value toward real browsing:

| Gap | Status | Effort |
|---|---|---|
| Nested element scrolling (`overflow:scroll/auto`) | partial | medium |
| `preventDefault` consumption on content clicks | partial | medium |
| Inline-element rects in the a11y tree | missing | medium |
| Scroll-into-view on Tab focus | missing | small |
| Computed `cursor` property exposure | missing | medium |
| Context-menu event + target resolution | missing | medium |
| Browser HTML form elements (value/submit/validation/events) | missing | large |
| DOM drag events; CSS paint long-tail (inset shadow, blend, filters, `::first-line`) | missing | large / medium |

## Next: nested element scrolling (this slice)

The highest-value genuine gap, and a **data-flow gap, not an algorithm gap**:
the paint walk translates descendants by `-offset` (`paint_emit.rs:850`) and the
hit walk maps the query point through `+offset` (`serval_lane.rs:450`) already,
both tested; the feature is dead only because `incremental.rs` always passes
`ScrollOffsets::default()`, so the per-element map is perpetually empty.

Slice (self-contained in serval-layout, no public-API break):

1. `IncrementalLayout` retains an `element_scroll: ScrollOffsets<Id>` field,
   parallel to how `Viewport` retains document scroll.
2. `scroll_at(dom, x, y, dx, dy) -> bool`: hit-test the point, walk hit -> root
   via the existing `clips_overflow()` predicate (the same walk `affordances_at`
   uses to detect `Scrollable`), find the nearest scrollable ancestor, clamp the
   new offset to `0..(content_size - inner)` (off the fragment, like the roster's
   `max_scroll`), write it into the map. No scrollable element -> the viewport
   (already works).
3. `emit_paint_list` / `hit_test` **merge** the retained `element_scroll` with
   the passed param, so meerkat's existing roster-scroll usage is unchanged and
   content documents get nested scroll for free.

Vertical (`oy`) first, then x. Host wiring is one line: wheel -> `scroll_at`.
The per-container scrollable-overflow region (rule 4: abs-pos / transformed
descendant overflow) is a refinement on `content_size`, a follow-on.
