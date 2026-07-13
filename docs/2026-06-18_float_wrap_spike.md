# Inline text wrap-around-floats — spike + first-cut design

**Date:** 2026-06-18. **Parent:** `2026-06-16_real_web_layout_fidelity_plan.md`
item 4. **Scope:** narrow inline line boxes around a float's exclusion region (a
paragraph wrapping to the right of a `float:left`, reclaiming full width below
it). Grounded by a four-part study (parley capability, genet float state, the
engine model, the `break_all_lines` seam).

**Status (2026-06-18): first cut landed.** Implemented exactly as designed
below — taffy gains `FloatContext::exclusion_bands` + `InlineFloatBand` +
`BlockContext::inline_exclusion_bands` (patch 0002, see `GENET_PATCHES.md`);
the box tree snapshots bands per inline-context leaf into `TextMeasureCtx`;
`text_measure`'s `break_and_align_floats` drives `Layout::break_lines()` with
per-line `set_line_x` / `set_line_max_advance`. Proven by
`genet-layout`'s `inline_text_wraps_around_left_float` (200px column, 60×40
`float:left`: lines above the float's 40px bottom start at x=60, lines below
reclaim x=0, all ending at the 200px edge). Full lib suite green (192). The
"Deferred" list below is the remaining backlog.

## The finding: it's parley wiring + float-band plumbing, not a new line-breaker

parley 0.10.0 already models per-line geometry. The incremental breaker
`Layout::break_lines() -> BreakLines` exposes `break_next()` plus, on
`BreakerState`, `set_line_x` / `set_line_max_advance` / `set_layout_max_advance`
(`parley .../line_break.rs:248,259,281`). Per-line values flow through
`finish_line` (`inline_min_coord = line_x`, `inline_max_coord = line_x +
line_max_advance`) into `align()`, so a float-narrowed line breaks *and* aligns
in its own box. So the driver loop is: per line, query the float band at the
line's y, `set_line_x` + `set_line_max_advance`, `break_next`. (Constraint:
`break_next` asserts `line_max_advance <= layout_max_advance`; floats only narrow,
so call `set_layout_max_advance(content_box_width)` first. Clamp defensively.)

The float-band function already exists: patched Taffy's
`FloatContext::find_content_slot(min_y, cb_insets, clear, after, min_width) ->
ContentSlot` (`support/patches/taffy/src/compute/float.rs:487`) returns the first
band at/below `min_y` wide enough for `min_width`, or `segment_id: None` with a y
below all floats when none fits — which *is* line-level downward clearance
(reclaim full width below the float), already implemented.

## The real work: thread float bands into the inline measure

Floats are placed by **patched Taffy** (the `float_layout` feature); genet only
forwards `float`/`clear` (`box_tree.rs:699-712`). A block establishing an inline
context is a childless Taffy leaf laid out via `compute_leaf_layout`
(`box_tree.rs:828`); its measure closure (`~832-839`) captures only
`tree/node/known/avail`. The parent's `block_ctx` (which owns the `FloatContext`)
is in scope at `compute_child_layout_inner` (`box_tree.rs:790,794`) but **not
forwarded into the closure**. Today the only Taffy→inline channel is the scalar
`available_space.width`. That channel is the plumbing to build.

**Chosen channel:** a band source on `TextMeasureCtx`, keyed by `taffy_id`.
Before the leaf measure, the block child loop computes the leaf's content-box top
(`location.y + border.top + padding.top`, BFC space) and snapshots the active
float bands from `block_ctx` (shifted by the block's `y_offset` into the leaf's
content-box space), stashing them under the leaf's `taffy_id`.
`measure_inline_content` looks it up; **absent ⇒ no active floats ⇒ the existing
scalar `break_all_lines` path runs unchanged** (so every existing inline test is
byte-identical).

Coordinate facts to honor: `FloatContext` is BFC-root space; subtract the block's
`y_offset` + content-box insets to reach leaf-local space; parley line y is
leaf-local (0-based). The likeliest real defect is a coordinate-offset bug, so
the test asserts on `inline_min_coord`/`inline_max_coord` directly (no paint
round-trip).

## First cut

**In scope:** one `float:left`; inline text in the same BFC wrapping to its
right; text reclaiming full container width below the float's bottom.

**Touch:**
- `support/patches/taffy/src/compute/float.rs` — a thin accessor returning the
  active exclusion bands (or per-line `find_content_slot`) in child space; wrapper
  over the existing `segments` walk. Note in `GENET_PATCHES.md`.
- `box_tree.rs` `compute_child_layout_inner` (~790) — snapshot bands + leaf
  content-box top into `TextMeasureCtx` before the leaf measure (828).
- `text_measure.rs` — `TextMeasureCtx` gains a `taffy_id → (leaf_top, bands)`
  field (cleared in `reset`); `break_and_align` (552) branches to a float-aware
  `break_lines()` driver when a band source is present, else the scalar path;
  thread it through `measure_inline_content` (420) at the leaf call sites (465,
  481) only — **not** the intrinsic min/max-content passes (float-aware breaking
  runs once, on the definite-width pass; intrinsic width ignores float position).
- `box_tree.rs` test beside `float_left_places_blocks_side_by_side` (1656):
  container 200px, `float:left` 60×40 at (0,0), paragraph after; assert lines with
  top y<40 have `inline_min_coord≈60`/`inline_max_coord≈200`, lines with y≥40 have
  `inline_min_coord≈0`, transition at the float bottom.
- `paint_emit.rs` — **no change** (reads back `layout.lines()`; per-line
  `inline_min_coord` flows into glyph x automatically).

## Deferred

Right floats (symmetric, the accessor returns both insets), multiple/stacked
floats, line-level clearance plumbing, mid-paragraph float anchoring (interleave
`place_floated_box` with the line loop), variable-line-height reflow-and-retry
(parley `BreakerState` is `Clone` with `revert_to`/`revert` for this),
`shape-outside`.

## Risks

Coordinate-offset bugs (BFC vs leaf-local) — the main one; assert on per-line
coords. The `break_next` width assertion — clamp `slot.width` to
`content_box_width`. No regression to block-float placement
(`float_left_places_blocks_side_by_side` is a separate block path; the inline
change is additive and default-off when no band source is present). parley
re-break cost is one extra `find_content_slot` per line on the final pass only.
