# Scope: z-index / stacking and the remaining form controls

Forward scope for the genet-as-host track after Stage 6 (scrolling). It covers
the no-z-index gap that scrolling surfaced, and the three open T2 form controls
(radio, textarea, slider). Each section gives the current state, what the work
needs, the done condition, relative size, and dependencies.

Grounding (verified against the tree, 2026-05-31):
- `paint_emit` walks the DOM pre-order and emits in document order; there is no
  stacking sort.
- stylo parses and computes `z-index` (`get_position().z_index`, a `ZIndex` of
  `Auto | Integer(i32)`), so the value is available but unused.
- The runner dispatches click (`dispatch_click`) and key (`dispatch_key`) only.
  There is no pointer-down / move / up.
- `text_field`'s `TextInput { text, caret, anchor }` is single-line. parley
  already lays the buffer out multi-line and `caret_rect` / `selection_rects`
  are line-aware, so the geometry side of multiline already works.

---

## 1. z-index / stacking

**Status: Tier 1 done (`e10a3211f82`).** Tier 2 deferred to CSS conformance.

Consequence that motivated it (seen in the demo): a `position: absolute` overlay
was emitted before a later sibling and painted under it, so the `select`
dropdown had to be ordered last to stay on top and clickable. Tier 1 fixes that.

### Tier 1, positioned-on-top (recommended now)

A two-pass emit. In-flow content paints in document order as today; then
positioned elements (`position != static`) paint after, ordered by `z-index`,
then document order as the tiebreak. An overlay paints above in-flow content
regardless of where it sits in the document, which removes the select-last
workaround and makes overlays, popups, and dropdowns robust.

- Done when: an absolutely-positioned box declared before a later in-flow
  sibling paints over it, and `z-index` orders two overlapping positioned boxes.
- Size: medium, engine-only (genet-layout `paint_emit`). The walk gathers each
  positioned subtree into a deferred list keyed by `(z-index, document-order)`,
  emits the in-flow pass, then emits the sorted list.
- Also touches hit-testing: `GenetLaneView` must resolve the topmost positioned
  element first so clicks match paint. Without this, paint and hit-test disagree
  (the bug we just worked around by ordering).

### Tier 2, full CSS painting order + stacking contexts (defer)

CSS 2.1 Appendix E order within each stacking context (negative z-index, block,
float, inline, positioned auto/0, positive z-index), with stacking contexts
established by positioned + z-index (later also opacity and transform). Correct
for nesting and the long tail.

- Done when: nested stacking contexts paint in CSS-conformant order.
- Size: large. This belongs with the CSS-rendering-conformance effort, not the
  host track. Tier 1 covers chrome.

---

## 2. radio (group)

**Status: done (`391af9f09d5`).**

The analogue of `checkbox` / `select`: state is which option is selected;
clicking one selects it and deselects the rest.

- Needs: a `RadioGroup` state (selected index) and a `radio_group(state,
  options)` view, one clickable element per option reflecting the selection via
  a class plus `role="radio"` / `aria-checked`. Reuses `on_click` and `lens`.
- Done when: clicking an option selects it and only it, reflected in state and
  aria, composable onto an app field via `lens`.
- Size: small. The `select` pattern without the dropdown.
- Dependencies: none. No engine work, no overlay, no drag.

---

## 3. textarea (multiline text)

**Status: done (`99998dae76a`).** Confirmed genet feeds raw text to parley,
which breaks at `\n`, so newlines render as line breaks with no engine work. Up
/ down navigate `\n`-delimited (hard) lines; soft-wrap visual-line navigation
(needs the layout) is the remaining refinement.

`text_field` was single-line; parley already lays the buffer out multi-line and
the caret geometry is line-aware, so the work was the model and key handling.

- Needs: Enter inserts `\n`; ArrowUp / ArrowDown move the caret to the adjacent
  visual line; Home / End scope to the current line. The vertical motion (an
  offset to the nearest offset on the line above or below) is the substance, and
  parley exposes the cluster / line navigation to compute it. The render path
  already paints multi-line, so this is not layout or paint work.
- Done when: a `textarea` control edits multiple lines, Enter breaks a line, and
  Up / Down move across lines with correct caret geometry.
- Size: medium.
- Dependencies: none.

---

## 4. slider / range

Blocked on events: the runner dispatches click and key only. A slider is a
pointer drag (press the thumb, move, release), which is pointer-down / move / up,
not a click.

### Part A, the pointer-drag foundation (the bulk)

New `pointerdown` / `pointermove` / `pointerup` events plus runner dispatch, and
host wiring of winit `MouseInput` press/release and `CursorMoved` into a drag
cycle with capture (the element that took pointerdown keeps receiving move and
up until release). This is the Lane H "more pointer events" axis, and it unlocks
more than sliders: scrollbar-thumb dragging, resize handles, and the Mere
drag-tab-out interaction.

### Part B, the control

A track and thumb. pointerdown on the thumb starts a drag, pointermove maps the
cursor position to a value in `[min, max]`, pointerup ends it. Small once Part A
exists.

- Done when: dragging the thumb changes the value continuously and the value
  drives the thumb position.
- Size: large, almost all in Part A; Part B is small.
- Dependencies: Part A. Worth landing on its own because it is reused widely.

---

## Recommended sequence

1. z-index Tier 1. **Done (`e10a3211f82`).**
2. radio. **Done (`391af9f09d5`).**
3. textarea. **Done (`99998dae76a`).**
4. Pointer-drag foundation, then slider. **Next.** Largest; the foundation is
   reusable for scrollbar-drag, resize, and drag-tab-out.

IME (the Mere-flip long pole) sits outside this scope; see the staging section
of `2026-05-27_genet_as_host_xilem_serval_plan.md`.
