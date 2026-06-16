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

## 1. UA-stylesheet completeness (highest value)

**Why first:** when the UA sheet is thin, every box is mispositioned before any
harder feature matters. A page with wrong `<h1>`/`<p>`/`<ul>` margins and
heading scale looks broken even if tables and floats were perfect. This is the
cheapest large visual win.

**State (updated 2026-06-16):** split into two halves once attempted.

- **Heading scale + weight — DONE.** `ua_defaults.rs` now ships the
  `<h1>`..`<h6>` `font-size` scale (2em … 0.67em) + `font-weight: bold`. These
  are *collapse-free* (font-size/weight change a box's content, not its
  margins), so the full and incremental layout paths agree. Verified by
  `box_tree.rs` `ua_heading_scale_makes_h1_taller_than_p`. Headings were
  previously body-sized and un-bolded.
- **Block-flow margins — BLOCKED on a margin-collapse-parity engine fix.** The
  spec margins (`body { margin: 8px }`, `p`/`h1`..`h6`/`ul`/`ol`/`blockquote`/
  `figure`/`pre`/`dd`) are correct in the *full* box-tree path (a stacked-`<p>`
  gap test confirmed adjacent-sibling collapse), but adding them to the UA
  sheet exposed two divergences that a sheet edit cannot fix:
  1. **Root-element margin.** The full box-tree root handling drops the root's
     child margin. Runtime probe (`box_tree` `lay`): with `body { margin: 8px }`,
     a `<div>` at the body origin lands at **(0, 0)**, not (8, 8) — body's own
     left margin is dropped and its top margin over-collapses through to the
     ICB. One level deeper a margined `<div>` *inside* body lands at **(8, 0)**:
     the left margin applies correctly, the top still collapses through. So the
     bug is specific to the **root element's child** (body), the classic
     "root-element margins don't collapse with the viewport" special case real
     browsers implement but serval doesn't yet. It is *not* the `body { width:
     100% }` over-constraint (auto-width probes the same (0,0)). `IncrementalLayout`
     applies body's margin, so the two paths also disagree on the gutter.
  2. **First-child margin collapse at the splice boundary.** In full-document
     layout a first child's top margin collapses *through* its block parent
     (html → body → p, so the p lands at y=0). `IncrementalLayout::apply_structural`
     (`incremental.rs:875`) re-lays-out the changed subtree in isolation via
     `SubtreeView::new(dom, root)`; a subtree root establishes its own
     formatting context, so that margin does NOT collapse through, and the
     first spliced child is mis-positioned relative to a full recompute
     (broke `incremental::tests::{inner_html_replace_splices_matching_full,
     structural_change_splices_incrementally}` the moment `p` had a margin).

  So "UA-sheet completeness" is NOT the self-contained, no-engine-change task it
  looked like: the block margins are gated on margin-collapse parity across the
  full and incremental paths.

**Slice (revised):**

1. Done: heading scale + weight (shipped).
2. Engine fix A — the root-element-margin special case in the full box-tree
   root handling: the root's child (body) margin must apply on all edges and
   not collapse through to the ICB (not a `width` issue; auto-width probes the
   same (0,0)).
3. Engine fix B — first-child margin collapse *through* a block parent in
   `IncrementalLayout`'s subtree splice (the `SubtreeView` boundary must carry
   the collapse context the subtree root sits in within the full document).
4. Only then: add the spec block margins to `ua_defaults.rs`, with the existing
   stacked-block geometry tests extended to first-child cases on both paths.

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

## 3. `white-space: pre` / `pre-wrap` preservation

**State:** `construct.rs:509` calls `collapse_whitespace` unconditionally on
text content, so source newlines and indentation always collapse to single
spaces. `white-space: nowrap` is handled (`construct.rs:163`, tested
`box_tree.rs:1221`), but the *preserve* values (`pre`, `pre-wrap`, `pre-line`)
are not. Code blocks, ASCII art, and `<pre>` content render wrong.

**Slice:** gate `collapse_whitespace` on the computed `white-space` value at the
`construct.rs:509` call site; for `pre`/`pre-wrap` preserve runs and honor
forced newlines (segment text on `\n` into separate inline items or carry the
break into the parley builder). Self-contained in `construct.rs` +
`text_measure.rs`. Add a test that a `<pre>` with two source lines lays out on
two lines.

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

1's heading half is done; its margin half is now the highest-value *engine*
work (the two margin-collapse-parity fixes), since margins shift every box and
unblock honest visual comparison. In parallel, 3 (`white-space:pre`, small and
self-contained) and 2 (tables, large but high-frequency) need no margin work.
4 (float wrap) and 6 (flex/grid hardening) are the precision passes. 5 and 7
are promoted by target shift (interactive pages; visual polish), not by being
next in line.

Most items are independently shippable and individually testable, and none
requires a planes-architecture change. The exception surfaced by 1: the UA
block margins are coupled to margin-collapse correctness across the full and
incremental layout paths, so they are an engine fix, not a sheet edit.
