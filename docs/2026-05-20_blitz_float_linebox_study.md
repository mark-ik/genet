# Study: how blitz-dom wraps text around floats

Read-only study (2026-05-20) of `blitz-dom 0.3.0-alpha.2`, to learn the
technique serval needs to turn block-level floats (shipped) into real
text-wrapping-around-floats. Companion to
`docs/2026-05-20_stylo_taffy_adoption_plan.md`, which documented the
"parley-leaf seam" limit.

## The seam, restated

serval lays out an inline formatting context as an **opaque Taffy leaf**:
`InlineContent` is measured by `measure_inline_content`, which calls
`parley::Layout::break_all_lines(Some(width))` with a *single fixed*
width. Taffy sees the leaf as a black box. A float (a sibling box) can't
reach into that leaf to vary the per-line width, so text can't wrap
around it. Block-level float displacement works (Taffy moves sibling
*boxes*); line-box intrusion does not.

## How blitz-dom does it

Three moves, all in `blitz-dom/src/layout/`:

1. **Own the tree; Taffy is just algorithms.** `BaseDocument` implements
   `taffy::LayoutPartialTree` over its own node store (`layout/mod.rs`)
   and calls `compute_block_layout` / `compute_flexbox_layout` /
   `compute_grid_layout` / `compute_leaf_layout` directly. It does **not**
   use `TaffyTree`. Inline is a first-class mode, `compute_inline_layout`
   (`layout/inline.rs`), not a measure closure — so inline layout has
   access to the surrounding block formatting context.

2. **A BFC float-tracker.** A `block_ctx` (block formatting context)
   threads through layout and exposes two operations:
   - `find_content_slot(y, clear, …) -> { x, width, y, segment_id }` —
     the available horizontal slot for content at vertical position `y`,
     **reduced by any floats intruding at that y**.
   - `place_floated_box(size, min_y, direction, clear) -> pos` — positions
     a float and registers it so later `find_content_slot` calls reflect
     it.

3. **parley's incremental break loop + `YieldData`.** Instead of
   `break_all_lines(width)`, blitz drives parley line-by-line:

   ```text
   let mut breaker = layout.break_lines();
   // seed first line from the BFC's slot at y=0
   let slot = block_ctx.find_content_slot(0.0, Clear::None, None);
   state.set_line_max_advance(slot.width);  state.set_line_x(slot.x); …
   while let Some(yield) = breaker.break_next() {
     match yield {
       LineBreak => {
         // query the BFC for the NEXT line's float-reduced slot
         let slot = block_ctx.find_content_slot(state.line_y(), …);
         state.set_line_max_advance(slot.width);  // <-- text wraps here
         state.set_line_x(slot.x);  state.set_line_y(slot.y);
       }
       InlineBoxBreak(b) => {
         // a float box hit mid-break: lay it out + register in the BFC
         let out = self.compute_child_layout(b.inline_box_id, float_inputs);
         block_ctx.place_floated_box(out.size + margin, state.line_y(),
                                     dir, clear);
       }
       MaxHeightExceeded(_) => { /* TODO upstream */ }
     }
   }
   ```

   Floats are pushed into parley as inline boxes flagged `break_on_box:
   true`, so parley yields `InlineBoxBreak` at the float and `LineBreak`
   at each line end. At every line break blitz re-asks the BFC for the
   float-reduced slot and sets parley's `line_max_advance` / `line_x` —
   **that per-line width variation is the text wrap.**

## Feasibility for serval

- **The parley API is already present.** serval's pinned **parley 0.9.0**
  exposes `Layout::break_lines()`, `BreakLines::break_next() ->
  Option<YieldData>` (incl. `YieldData::InlineBoxBreak`), and the state
  setters `set_layout_max_advance` / `set_line_max_advance` /
  `set_line_x` / `set_line_y` (`parley/src/layout/line_break.rs`). No
  parley bump needed. (blitz gates this behind its own `floats` feature;
  the parley surface is unconditional in 0.9.0.)

- **The architectural cost is the real cost.** The seam is tied to using
  `TaffyTree<InlineContent>` with a measure closure. A leaf measure
  cannot see sibling-float state, so text-wrap-around-float is impossible
  *as long as inline content is a Taffy leaf*. To adopt the technique
  serval must move inline layout into a float-aware break loop with BFC
  access. Two shapes:
  - **(a) Own the tree (blitz shape).** Implement `LayoutPartialTree`
    over the planes (StylePlane/FragmentPlane/…), call taffy's
    `compute_*` directly, add a `compute_inline_layout` + a BFC
    float-tracker. Biggest change; gives full control.
  - **(b) Special-case float BFCs.** Keep `TaffyTree` for block/flex/grid,
    but when a block establishes a BFC that contains floats, route it to
    a custom inline+float pass (the break loop + BFC) outside Taffy's
    leaf measure. Narrower, but two code paths to reconcile.

## Strategic read

This is the Blitz-convergence fork made concrete. Adopting the
*technique* pulls serval's layout toward blitz-dom's exact shape
(own-tree `LayoutPartialTree` + BFC + parley-yield). The **planes
architecture** (NodeId-keyed StylePlane/FragmentPlane/ImagePlane + query
traits) is the main thing that still differs — blitz-dom is a Node-tree.
So the honest question once this lands on the roadmap: do we reimplement
blitz's inline/float layout *over the planes*, or has serval's layout
converged enough that consuming blitz-dom (and keeping planes as a
query/view layer on top) is the lower-maintenance path? Decide that
before building the own-tree pivot, not after.

## Pointers

- `blitz-dom/src/layout/mod.rs` — `LayoutPartialTree for BaseDocument`,
  the `compute_*` dispatch.
- `blitz-dom/src/layout/inline.rs` — `compute_inline_layout`, the
  `break_next()` / `YieldData` loop, float placement (≈ lines 440–525).
- `parley/src/layout/line_break.rs` — `YieldData`, `BreakLines`, state
  setters.
- serval today: `serval-layout/text_measure.rs`
  (`measure_inline_content` → `break_all_lines`) is the seam to replace.
