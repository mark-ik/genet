# A spec-owned box tree and formatting contexts for Livery

**Date:** 2026-07-26
**Status:** scoped, not started. Founded by Mark's boundary audit of the
same day.
**Decision record:** Mark, 2026-07-26: "Insert a spec-owned `CssBoxTree`
and formatting-context layer between computed values and Taffy. Taffy
remains the layout backend. CSS owns box generation, anonymous wrappers,
table structure, intrinsic sizing, positioning categories, and used-value
resolution."
**Parent:** the
[cutover plan](./2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md),
whose F4 bar this does not change and whose conformance ledger it exists to
move.

## The defect

Livery was designed as a bounded engine for one lane: Cambium's structural
UI. Its lib.rs says so, and the bounded property set was the right call for
that job. The cutover has been promoting it to fullweb by adding property
names and closing a differential against Stylo, without replacing the
bounded semantic models underneath.

`genet-livery` has no CSS box tree and no formatting-context layer. Computed
values lower almost directly into Taffy styles, and where the model cannot
represent a CSS semantic it collapses onto the nearest backend primitive:

| CSS semantic | what Livery does today |
|---|---|
| `display: table` / `table-row` | grid container / flex row |
| most `display` values | block |
| `position: fixed` | absolute |
| `position: sticky` | relative |
| `min-content` / `max-content` sizes | `auto` |
| anonymous box fixup | absent |
| formatting-context roots | implicit, never modelled |

CSS Display 3 and CSS Sizing 3 make those distinctions foundational, not
presentational. A backend mode is not a box role.

**The table guard is the proof that the lowering happens too early.** Rows
are discarded during tree construction, before positioning is resolved, so a
`position: relative` row has nothing left to apply its offset to and the
implementation needs a fallback that turns the whole feature off for that
table. No amount of care inside the current shape fixes that; the
information is gone before the question is asked.

## The shape

One new layer, owned by the specifications, between computed values and
Taffy:

```
DOM + computed values  ->  CssBoxTree  ->  Taffy tree  ->  fragments
                           ^^^^^^^^^^
                           box generation, anonymous fixup,
                           formatting contexts, intrinsic sizing,
                           positioning categories, used values
```

Taffy stays the layout backend and keeps doing what it is good at: flex,
grid, and block track sizing. It stops being asked to *be* the box model.

**What the box tree owns.**

- **Box generation.** `display` maps to an outer role (block-level,
  inline-level, none) and an inner role (flow, flow-root, flex, grid,
  table). Two boxes with the same Taffy display can differ in both.
- **Anonymous box fixup.** Block containers with mixed children, inline
  boxes split around blocks, and the full table fixup chain of CSS Tables 3
  section 2: missing rows, row groups, and cells are generated rather than
  assumed present in the markup.
- **Formatting-context roots.** Which boxes establish a BFC, IFC, flex, grid
  or table formatting context, so float and margin-collapse behaviour follow
  from the model instead of from special cases.
- **Table structure.** Rows, row groups, columns, column groups, captions,
  and cells keep their roles through construction. Spans are distributed
  over a real cell grid. The grid backend can then size tracks without the
  structure having been thrown away first.
- **Intrinsic sizing.** `min-content`, `max-content`, and `fit-content()`
  are carried and resolved, rather than collapsed to `auto` on the way in.
- **Positioning categories.** Static, relative, absolute, fixed, and sticky
  stay distinct, each with its own containing-block rule.
- **Used-value resolution.** The single place CSSOM asks for used values,
  replacing the handwritten per-property table described in the parent
  plan's findings.

## Stages, each with a receipt

- **B0 - the tree, with today's behaviour.** Build `CssBoxTree` and lower it
  to the same Taffy tree Livery builds now. No behaviour change; the receipt
  is that the whole reftest and testharness corpus is unmoved, proving the
  layer is a faithful refactor before anything rides on it.
- **B1 - box generation and anonymous fixup.** Outer/inner display roles and
  the anonymous-wrapper rules, including the block-flow whitespace rule the
  parent plan currently has to scope to flex and grid because the emulation
  depends on the wrong behaviour. Receipt: that scope restriction is lifted
  and CSS2 does not regress.
- **B2 - formatting contexts.** BFC/IFC roots, float containment, and
  margin collapsing derived from context rather than from ad-hoc checks.
- **B3 - table structure.** The CSS Tables 3 fixup chain, real row and
  column roles, spans, captions, and border-conflict resolution. Retires the
  positioned-row guard and the `partial` marker on `table-layout`.
- **B4 - intrinsic sizing.** Carry and resolve the content keywords, ending
  the quiet `min-content` = `max-content` = `auto` collapse.
- **B5 - positioning categories.** Real `fixed` and `sticky`, each with its
  own containing block.
- **B6 - used values for CSSOM.** One resolution point, generated from the
  CSSOM resolved-value rules rather than a handwritten list of properties.

## Sequencing against the cutover

This plan does not block F4. The differential can keep closing while the box
tree is built, and B0 is explicitly a no-op refactor so it can land at any
time.

It does bound what F4 means. Passing F4 with the current lowering proves
Livery can replace Stylo, not that Livery implements CSS; the parent plan's
ledger split says the same thing from the other side.

**Do B0 and B1 before more capability slices.** B1 lifts a restriction the
parent plan measured at 131 files, and every capability added before the
tree exists is written against a model that is about to change.

## Non-goals

- Replacing Taffy. It stays the layout backend for flex, grid, and block.
- A second layout engine. This is one layer, not a fork of the lane.
- Blocking the cutover on completion. F4 has its own bar and keeps it.

## Done condition

Livery's layout input is a box tree whose shape is derived from the CSS
specifications rather than from Taffy's capabilities, the deferral register's
two compounding entries are closed, and `display`'s catalog entry can state
the values it implements without the grammar overstating them.
