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

## 2. Tables

**State:** rendered as block. `ua_defaults.rs:64-66` forces
`table,caption,thead,tbody,tfoot,tr {display:block}`, and `box_tree.rs:659-668`
dispatches only `Block`/`Flex`/`Grid` with no `Display::Table` arm. Note that
`CssStyle::is_table` *is* forwarded (`box_tree.rs:593-595`), so the block-item
machinery already knows which boxes are tables; what is missing is the table
layout algorithm (column sizing, row/cell box generation, border-collapse).

**Slice:** this is the largest single item. Sequence it as: (a) stop forcing
`display:block` on table elements in the UA sheet; (b) add a `Display::Table`
dispatch arm; (c) drive it through Taffy's grid layout as the column/row engine
(the pragmatic path, since grid is already wired) or a dedicated table pass if
grid's model proves too lossy for `colspan`/`rowspan`. Decide (c) with a spike
before committing; do not assume grid-as-table is free.

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
`LayoutFlexboxContainer`/`LayoutGridContainer` impls at `box_tree.rs:823-844`)
but their measurement correctness on real layouts past the wired path is
unproven. Wired is not the same as faithful.

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

1 (UA-sheet metric defaults) and 3 (`white-space: pre`) are **done**. Next: 2
(tables, large but high-frequency) is the biggest remaining gap. 4 (float wrap)
and 6 (flex/grid hardening) are the precision passes. 5 and 7 are promoted by
target shift (interactive pages; visual polish), not by being next in line.

Each item is independently shippable and individually testable, and none
requires a planes-architecture change. 1's lesson for the rest: a UA-sheet
change that moves boxes can surface latent incremental-layout assumptions (here,
the splice's absolute-vs-parent-relative coordinate handling and its BFC-at-the-
boundary margin collapse), so re-run the `paint` e2e band scans after any future
metric-default change.
