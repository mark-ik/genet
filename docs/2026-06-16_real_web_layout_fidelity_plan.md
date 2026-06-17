# Real-web layout fidelity plan

**Date:** 2026-06-16. **Parent:** `2026-06-16_serval_layout_roadmap.md` (Thread
1). **Scope:** the gap between "serval-layout lays out correctly for the
constructs it models" and "serval-layout renders a typical real page
faithfully." Each item below is verified against file:line, ranked by how much
a real page improves per unit of work. This is a secondary plan so the roadmap
need not carry it.

The engine itself is sound: `box_tree.rs` implements Taffy's container traits
directly, block/flex/grid dispatch is wired, inline formatting and replaced
boxes work, hit-testing and scroll are done. The fidelity gaps are *missing
models*, not engine bugs.

---

## 1. UA-stylesheet completeness — DONE (2026-06-16)

**Why first:** when the UA sheet is thin, every box is mispositioned before any
harder feature matters. A page with wrong `<h1>`/`<p>`/`<ul>` margins and
heading scale looks broken even if tables and floats were perfect. This was the
cheapest large visual win.

**Shipped.** `ua_defaults.rs` now carries the metric defaults: the `<body>` 8px
gutter, the `<h1>`..`<h6>` `font-size` scale (2em … 0.67em) + `font-weight:
bold`, and the block-flow margins (`p`/`h1`..`h6`/`ul`/`ol`/`blockquote`/
`figure`/`pre`/`dd`/`hr`). Verified end to end: `box_tree.rs`
(`ua_heading_scale_makes_h1_taller_than_p`, `ua_body_gutter_offsets_the_body_box`,
`ua_paragraph_margins_collapse_between_siblings`) and the 19 `paint`
`html_to_pixels_e2e` tests (three fixtures gained a `body { margin: 0 }` to
neutralize the new gutter for their pixel coordinates).

**What it took (the engine story — corrected from the initial guess):**

- The block margins are *correct in the full box-tree path* with no engine
  change. The earlier "root-element margin is dropped" finding was a **probe
  bug**: fragment `location` is parent-relative (Taffy's `final_layout.location`;
  `caret::absolute_origin` walks to accumulate), so the original probe read a
  child's body-relative `(0,0)` as if absolute. Re-probed, `<body>` sits at
  `(8,8)` relative to `<html>` exactly as it should. There was no "fix A".
- The one real fix was **`IncrementalLayout`'s splice (`apply_structural`)**, in
  two parts:
  1. **Margin-collapse parity.** `SubtreeView::new(dom, root)` makes the spliced
     subtree root the scoped ICB — a BFC — so its first/last in-flow child
     margins stop collapsing *into* it the way a non-BFC root collapses them in
     the full document. New guard `splice_loses_margin_collapse` detects this
     (root not an independent formatting context AND a first/last in-flow child
     carries a collapsing block margin) and falls back to `full_relayout` (cheap
     for a shallow root like `<body>`). Test:
     `margined_first_child_falls_back_to_full`.
  2. **Coordinate space.** The splice translated every descendant by the root
     delta (`prior_root - scoped_root`), which forced descendants into *absolute*
     space while the full path stores *parent-relative*. Dead code while `<body>`
     sat at the origin; the moment the UA gutter offset `<body>`, spliced
     children diverged by the gutter. Fixed: descendants keep their scoped
     parent-relative locations; only the subtree root pins to its prior location.

  The two existing splice tests now use `overflow: hidden` bodies (a BFC) so
  they keep exercising the splice path under the UA `p` margin instead of taking
  the collapse fallback.

**Deferred (small):** nested-section heading rescaling (`:is(article,…) h1`).

---

## 2. Tables — first cut DONE (2026-06-16)

**Shipped: simple tables lay out as a grid.** A `<table>` previously stacked
every cell as a block; it now renders as a 2D grid of its cells.

How it works (grounded in code):

- `stylo_taffy::convert::display` already maps `display: table` ->
  `taffy::Display::Grid` and `table-cell` -> `Block` (registry
  `convert.rs:179,183`); Taffy has no table algorithm of its own, so
  table-as-grid is the design.
- **Prerequisite:** CSS grid was non-functional (grid track properties gated
  behind servo's `layout.grid.enabled`, unset, so authored templates dropped to
  `None` and grids collapsed to one column). Enabled in `cascade.rs`
  `enable_serval_properties`; verified by `grid_template_lays_out_cells_in_a_grid`.
- **UA sheet:** replaced the forced `display:block` on table elements with their
  real display (`table` / `table-row-group` / `table-row` / `table-cell` /
  `table-caption`; `th` bold).
- **Box-tree flattening:** HTML nests cells `table > (thead/tbody/tfoot) > tr >
  (td/th)` (html5ever auto-inserts `<tbody>`), but Taffy grid lays out only a
  container's *direct* children. `box_tree.rs` `build_table` + `collect_table_rows`
  walk the row-group / row nesting and make the cells **direct grid children** of
  the table box, each tagged with its 0-based `(row, col)`.
- **Placement injection (no hard container-template needed):** each cell carries
  `grid_placement` on its `BoxNode`; `CssStyle` (the grid item style) implements
  `GridItemStyle::grid_row`/`grid_column` to report an explicit grid line at
  `row+1` / `col+1`. The table's *implicit* grid then auto-sizes the column/row
  tracks to cell content. (This sidesteps `GridContainerStyle`, whose
  associated track-list iterator types are bound to `TaffyStyloStyle` and would
  be awkward to synthesize.) Verified by `table_cells_lay_out_in_a_grid` (a 2x2
  table -> four cells at the four grid positions) + paint e2e green.

**Deferred (real, recorded):** `colspan`/`rowspan` (map to grid spans via the
same per-cell injection, `end: line(start + span)`); `border-collapse`; the auto
vs fixed table-layout column-width-distribution algorithm (the implicit grid's
`auto` tracks approximate auto-layout, but not its min/max content redistribution
rules); `<caption>` (currently walked-past, so caption content does not render);
`<colgroup>`/`<col>` column styling; ragged rows / cells-per-row mismatch (each
cell is placed explicitly, so ragged tables place correctly, but missing cells
leave grid holes rather than CSS table's anonymous-cell fill).

---

## 3. `white-space: pre` / `pre-wrap` preservation — DONE (2026-06-16)

**Shipped.** The text-gathering path no longer collapses unconditionally: it
reads the text's computed `white-space-collapse` and applies it
(`construct.rs` `apply_white_space_collapse`). `Collapse` (the `white-space:
normal` / `nowrap` default) folds whitespace runs to one space as before;
`Preserve` / `BreakSpaces` (`pre` / `pre-wrap`) keep whitespace + newlines
verbatim, and each source `\n` becomes a parley line break (the same mechanism
`<br>` already used); `PreserveBreaks` (`pre-line`) collapses spaces but keeps
newlines. The UA sheet gains `pre { white-space: pre }` (the shorthand also sets
`text-wrap-mode: nowrap`, so `<pre>` lines don't soft-wrap, riding the existing
`no_wrap_of` path). Verified: `box_tree.rs`
(`pre_preserves_newlines_as_line_breaks` + a `normal_whitespace_collapses_newlines`
control) and the 19 `paint` e2e tests (normal-text collapse unchanged).

**Deferred (small):** leading/trailing per-line whitespace trimming edge cases;
a monospace default font-family for `<pre>` (depends on a registered monospace
face); `tab-size`. The core preserve-newlines behavior that makes code blocks
and ASCII art legible is in.

---

## 4. Inline text wrap-around-floats

**Precise gap:** block-level float *placement* is done. Two `float:left` divs
sit side by side (tested `box_tree.rs:1355`
`float_left_places_blocks_side_by_side`), via the `CssStyle` float/clear
forwarding (`box_tree.rs:597-598`). What is missing is **inline text shaping
around a float's exclusion region**: parley line breaking
(`text_measure.rs:553` `break_all_lines`) takes a single `max_advance` and does
not narrow individual lines to avoid a floated box's rect.

**Slice:** this is the hardest of the wrap items and worth a focused study of
how a mature engine threads float exclusion rects into line breaking (the
Blitz study referenced in the roadmap is the reference point). Likely shape:
collect the active float rects for the current block, and feed parley a
per-line available-width sequence instead of one `max_advance`. Treat as a
spike first; the seam is `text_measure.rs` `break_all_lines`.

---

## 5. Engine-rendered form controls

**State:** host-side form controls exist (xilem-serval, per the 2026-06-14
audit) and the host composites them. What is missing is *engine-rendered*
controls (the box tree generating and painting `<input>`/`<button>`/`<select>`
geometry itself), which real pages need when controls are styled by author CSS
or sit inside content that the host does not own.

**Slice:** large; cross-references `2026-05-31_zindex_form_controls_scope.md`.
Lower priority than 1 to 4 for read-mostly browsing; promote when interactive
pages become the target.

---

## 6. flex / grid measurement correctness

**State:** flex and grid are *dispatched* (`box_tree.rs:666-667`,
`LayoutFlexboxContainer`/`LayoutGridContainer` impls at `box_tree.rs:823-844`).
**Grid was silently broken** until 2026-06-16: `grid-template-columns/rows` are
servo-pref-gated (`layout.grid.enabled`), unset, so the cascade dropped them and
grid collapsed to a single column. Now enabled (see item 2), with a 2x2
`grid-template` test. Flex (and the rest of the grid surface: `repeat()`,
`fr`, `minmax`, gaps, alignment, auto-placement) is still wired-but-unproven on
real layouts. Wired is not the same as faithful; check whether any *other* grid
or flex sub-properties are pref-gated the same way.

**Slice:** build a fidelity test set of real-world flex/grid patterns (nav bars,
card grids, holy-grail layouts) and assert geometry; fix divergences in the
container-style forwarding. No new dispatch, mostly correctness hardening.

---

## 7. Paint tail

**State:** the long tail of paint features named in the 2026-06-14 audit: inset
shadow, blend modes, filters, `::first-line`. Each is bounded and independent.

**Slice:** pick up opportunistically; none blocks the higher items. Inset shadow
and `::first-line` are the most commonly hit on real pages.

---

## Suggested order

1 (UA-sheet metric defaults), 3 (`white-space: pre`), and 2 (tables, first cut)
are **done**; grid (item 6's core) is enabled too. Next: 4 (float wrap) and the
table long-tail (spans / `border-collapse` / `<caption>`). 6 (flex hardening,
and proving the rest of the grid surface) and 5/7 (form controls, paint tail)
follow by target shift.

Each item is independently shippable and individually testable, and none
requires a planes-architecture change. 1's lesson for the rest: a UA-sheet
change that moves boxes can surface latent incremental-layout assumptions (here,
the splice's absolute-vs-parent-relative coordinate handling and its BFC-at-the-
boundary margin collapse), so re-run the `paint` e2e band scans after any future
metric-default change.
