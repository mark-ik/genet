# serval-layout: state and roadmap

**Date:** 2026-06-16. **Scope:** the `serval-layout` crate (the engine that
turns a styled DOM into a fragment tree and a paint list). This is the
high-level map. It points at, and does not restate, the audits and scopes that
already cover individual subsystems. Two follow-on plans carry the heavy
threads:

- `2026-06-16_real_web_layout_fidelity_plan.md` (layout fidelity on real pages)
- `2026-06-16_element_view_and_scripted_tier_plan.md` (external-texture element
  view + the scripted tier)

This doc stays above both so the roadmap is not just those two threads.

---

## What serval-layout is

A flat crate, ~26k LOC across 28 modules, 183 inline tests, no `TODO` markers.
It is the live layout engine for both content lanes (the document lane and the
HTML/serval lane). The planes model from
`2026-05-17_serval_layout_planes_architecture.md` is realized: DOM / Style /
Layout / Fragment / Paint planes keyed by `D::NodeId`, with the Stylo firewall
holding (Stylo traits appear only in `adapter_stylo`).

`box_tree.rs` is the engine. It implements Taffy's `LayoutPartialTree` /
`LayoutBlockContainer` / `LayoutFlexboxContainer` / `LayoutGridContainer`
traits directly against serval's own box tree; the old `cv_to_taffy` oracle is
retired and parity is held by inline tests rather than a shadow tree. Paint
emits a `ServalPaintList` (`paint_emit.rs`); inline formatting, replaced boxes
(`<img>` and `<external-texture>`), and text run geometry come through parley.

The authoritative subsystem docs, none of which this roadmap duplicates:

- `2026-06-02_serval_holistic_audit.md` — cross-subsystem state.
- `2026-06-07_serval_layout_infrastructure_scope.md` — interaction state,
  cascade-time fonts, quirks mode, pseudo-elements. **All four are done.**
- `2026-06-14_engine_capability_audit.md` — hit-testing and browser-readiness,
  re-grounded against file:line. This is the most current capability ledger.
- `2026-05-17_serval_layout_planes_architecture.md` — the planes model.

---

## Where it is strong (done, verified)

These are settled. The roadmap does not reopen them; it builds on them.

- **Box generation + block/flex/grid dispatch** through the real Taffy trait
  impls (`box_tree.rs:659-668`). Flex and grid are wired and dispatched.
- **Inline formatting + replaced boxes.** `<img>` and `<external-texture>` lay
  out as replaced boxes positioned by parley; rects are recoverable for
  hit-testing and link harvest.
- **Hit-testing, including inline boxes** (`inline_hit.rs`) and
  `pointer-events` (`2026-06-14_engine_capability_audit.md:10-21`). A
  `display:inline` element with no box of its own resolves through its IFC
  leaf's parley runs, standards-correct (CSS2.2 §9.4.2), never a union box.
- **Document and viewport scroll**, including root->viewport overflow
  propagation with `<body>` fallback, `Fixed != Absolute`, `%`-height chains,
  `vw`/`vh` + resize re-resolution.
- **Nested element scrolling** (`overflow:scroll/auto`) via
  `IncrementalLayout::scroll_at` with CSS scroll chaining
  (`2026-06-14_engine_capability_audit.md:57-68`).
- **Selection + caret geometry**: single- and multi-node (`caret::range_rects`,
  `selection_rects`, `caret_byte_at_point`), consumed live by meerkat for HTML
  selection and find-in-page.
- **Focus model + Tab order**, click-to-focus, Enter/Space activation.
- **text-decoration** (all three lines), interaction state, cascade-time fonts,
  quirks mode, pseudo-elements (`::before`/`::after` + the followups in
  `2026-06-11_pseudo_element_followups_scope.md`).
- **Find-in-page primitive** (`caret::find_text_rects`,
  `find_text_rects_from_layout_dom`) wired into meerkat's content actor
  (committed serval `28e3e55`, mere `eefbed6`).
- **External-texture element view** end to end: the `<external-texture key>`
  element, its box-tree replaced-box participation, the
  `DrawExternalTexture` paint pass, and host compositing. See the element-view
  plan; the serval side is done, the open work is meerkat's render-loop swap.
- **Multi-root documents** (2026-07-11, `f7b3c53`): a host-built synthetic
  DOM may hang several element children directly off the document node (no
  `<html>` wrapper — merecat's chrome layer, widget pools). Box tree wraps
  them in a synthetic block root, the cascade traverses every root, and
  root-background propagation gates on a sole root. Before this, everything
  after the first element child silently emitted nothing — worth remembering
  as a class: "the document's first element child is the root" is a parsed-
  HTML assumption, not a `LayoutDom` invariant (the trait's `document()` doc
  now states the contract). Regression test:
  `multi_root_document_paints_every_root_element`.

---

## Where it is honest about gaps

Two threads carry real, ranked work. They are spun out so this roadmap stays a
map, not a backlog.

### Thread 1 — real-web layout fidelity

The engine lays out *correctly* for the constructs it models; it does not yet
model enough of them to render a typical real page faithfully. The ranked gaps,
detailed with file:line in `2026-06-16_real_web_layout_fidelity_plan.md`:

1. **UA-stylesheet completeness** (margins, heading scale) — **DONE
   (2026-06-16)**. The `<body>` gutter, heading scale + weight, and block-flow
   margins now ship in `ua_defaults.rs`. This required fixing the incremental
   splice's margin-collapse parity and an absolute-vs-parent-relative coordinate
   bug it had been hiding (see the fidelity plan). It was the highest-value
   single fix: a thin UA sheet shifted *every box*.
2. **Tables** — **first cut DONE (2026-06-16)**. A `<table>` now lays out as a
   grid of its cells (table-as-grid; `box_tree.rs` `build_table` flattens the
   row-group/row nesting into direct grid items, each with injected
   `grid-row`/`grid-column`). CSS grid was unblocked this session
   (`layout.grid.enabled`). Deferred: `colspan`/`rowspan`, `border-collapse`,
   table-layout width distribution, `<caption>`. See the fidelity plan.
3. **`white-space: pre`/`pre-wrap`** preservation — **DONE (2026-06-16)**. The
   text path applies the computed `white-space-collapse` (`construct.rs`
   `apply_white_space_collapse`); `<pre>` preserves whitespace + newlines and
   does not soft-wrap.
4. **Text wrap-around-floats.** The `break_all_lines` seam in `text_measure.rs`
   does not yet shape around float exclusion rects.
5. **Engine-rendered form controls** (vs the host-side controls that exist
   today).
6. **flex/grid measurement** correctness past the wired dispatch.
7. The **paint tail** (inset shadow, blend, filters, `::first-line`).

### Thread 2 — element view + scripted tier

Tracked in `2026-06-16_element_view_and_scripted_tier_plan.md`. From
serval-layout's seat the engine-side primitive (the external-texture replaced
box and its paint pass) is done; the scripted tier's layout coupling routes
through `IncrementalLayout` + `serval_scripted::relayout_if_dirty`, and pelt V4
drives a full page's inline `<script>` against a live DOM into relayout. The
open work there is consumer wiring (meerkat's render-loop swap) and the
scripted-DOM breadth, not a serval-layout engine primitive.

---

## Near-horizon, smaller threads (not promoted to their own plan)

These are real but bounded; recorded here so they are not lost.

- **`position: fixed` resolves its insets against the *parent*, not the viewport**
  (found 2026-07-10 by the WPT input-path work; diagnosed precisely, blocking 4
  WPT tests today). A fixed box's containing block must be the **viewport** (the
  ICB); serval uses the Taffy parent. Probe: for
  `#d { position: fixed; top:0; right:0; bottom:0; left:0 }` inside a normal
  `<body>`, `#d` computes to **`(0, 0, 800, 0)`** — the width resolves correctly
  from `left:0`+`right:0`, but the height is **0**, because the parent `body` has
  `height: auto` = 0 (its only child is out of flow). Give `body` an explicit
  `height: 600px` and the very same box resolves to `(0, 0, 800, 600)` and
  hit-tests correctly. So inset-derived sizing *works*; only the containing block
  is wrong.

  **The scope is larger than "fix a mapping", and this is the part to know before
  starting.** serval-layout has *no* fixed-vs-absolute handling at all: it hands
  stylo's computed style to `stylo_taffy::TaffyStyloStyle` (`box_tree.rs`) and
  inherits Taffy's absolute positioning wholesale, which is **parent-relative by
  construction**. There is no containing-block concept in the crate to correct —
  grep finds none. So `position: fixed` and `position: absolute` are today
  *indistinguishable*, and both resolve against the Taffy parent rather than the
  CSS containing block (nearest positioned ancestor for `absolute`, the ICB for
  `fixed`). Closing it means **introducing** the containing-block concept:
  hoisting fixed boxes to the ICB and resolving `absolute` against the nearest
  positioned ancestor. A thread, not a one-liner.

  *(Corrected same day, after reading `paint_emit`/`paint_stacking`: the
  paint-and-scroll half is already built. `is_fixed` / `is_out_of_flow` lift
  out-of-flow boxes into deferred stacking layers, and a fixed layer already
  counters the document scroll so it stays pinned, per the zindex scope doc's
  rule 3. So the earlier "with paint-order and scroll consequences" worry is
  largely pre-paid; the remaining work is confined to the layout tree, i.e. to
  which Taffy node an out-of-flow box parents under.)*

  Blocks `dom/events/non-cancelable-when-passive`'s four
  `wheel`/`mousewheel`-on-`div` tests (that div is a full-viewport fixed overlay),
  which are otherwise ready to pass; see the harness plan's H9. Note the wider
  exposure: any page whose chrome is a fixed overlay (a sticky header, a modal
  backdrop, a toast layer) mis-sizes the same way whenever its parent's height is
  auto — this is not a WPT-only curiosity.
- **Document-lane find.** Find-in-page is wired for the HTML lane; the document
  lane (gemtext/markdown -> retained `DocumentRenderPacket`) returns empty
  matches. Closing it means mapping glyph runs back to char offsets on the
  packet. A bounded, self-contained next thread.
- **Inline-element rects in the a11y tree.** Block-level bounds are present;
  inline runs are not yet exposed
  (`2026-06-14_engine_capability_audit.md:50`).
- **Scroll-into-view on Tab focus**; **computed `cursor` exposure**;
  **context-menu event + target resolution**
  (`2026-06-14_engine_capability_audit.md:51-53`).
- **`preventDefault` consumption on content clicks** (partial today).
- Scrollable-overflow region precision (rule 4: transformed / negative-margin /
  out-of-clip `absolute` descendants), recorded in the 2026-06-14 audit as a
  refinement on `scroll_extent`.

---

## Reading order for someone new to the crate

1. This roadmap (the map).
2. `2026-06-14_engine_capability_audit.md` (the current capability ledger).
3. `2026-05-17_serval_layout_planes_architecture.md` (the model).
4. The two follow-on plans, for the threads they own.

Then `box_tree.rs` (the engine), `construct.rs` (box generation),
`paint_emit.rs` (paint), `caret.rs` (selection/find), `inline_hit.rs`
(hit-testing).
