# A spec-owned box tree and formatting contexts for Livery

**Date:** 2026-07-26
**Status:** absorbed on 2026-07-26 by the
[Buckram CSS layout-engine plan](./2026-07-26_buckram_css_layout_engine_plan.md).
B-1 and B0 remain completed receipts; their one-box-to-one-rect output model
is reopened by Buckram K0.
**Historical decision record:** Mark, 2026-07-26: "Insert a spec-owned
`CssBoxTree` and formatting-context layer between computed values and Taffy."
The same-day fragment audit superseded "Taffy remains the layout backend":
Buckram owns layout and fragments, and calls Taffy's low-level flex/grid
algorithms.
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
DOM + computed values  ->  CssBoxTree  ->  formatting contexts
                           ^^^^^^^^^^             |
                           box generation,       v
                           anonymous fixup,   Taffy tree -> fragments
                           roles + provenance                  |
                                                               v
                                            ResolvedValueResolver -> CSSOM
```

Taffy stays the layout backend and keeps doing what it is good at: flex,
grid, and block track sizing. It stops being asked to *be* the box model.

**What the box tree owns.**

- **Box generation.** `display` maps to an outer role (block-level,
  inline-level) and an inner role (flow, flow-root, flex, grid, table),
  separately from list-item and internal-table roles. `none`, `contents`,
  replaced content, pseudo-elements, and anonymous origins remain explicit.
  Two boxes with the same Taffy display can differ in any of these.
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
- **Containing-block relationships.** A box records the relationship; the
  formatting-context algorithm applies the sizing and positioning rules.
- **Intrinsic sizing.** `min-content`, `max-content`, and `fit-content()`
  are carried into the responsible formatting context and resolved there,
  rather than collapsed to `auto` on the way in.
- **Positioning categories.** Static, relative, absolute, fixed, and sticky
  stay distinct, each with its own containing-block rule.

The tree does **not** own used values. It does not yet have layout results.
`ResolvedValueResolver` is a post-layout consumer of computed styles and
fragments, and is the single place CSSOM asks for used values. This replaces
the handwritten per-property table described in the parent plan's findings
without making a pre-layout structure answer a post-layout question.

`CssBoxTree` belongs in `genet-livery`, or in a later layout crate with the
same boundary. It does not belong in `livery`: the style engine produces
computed values; the browser integration owns box generation, layout,
fragments, paint provenance, hit testing, accessibility geometry, and CSSOM
used values.

## B-1 - identity before construction

CSS box generation is not one DOM node to one box, and layout is not one box
to one fragment. B0 must not preserve those two current storage assumptions
inside a nicer wrapper.

The contract lands before the tree builder:

- `BoxId`: stable identity within one generated tree.
- `BoxOrigin`: `Element`, `Text`, `Pseudo`, or `Anonymous`, with the
  originating DOM node retained wherever one exists.
- `DisplayRole`: outside role, inside role, list-item flag, internal-table
  role, and box suppression represented separately. Unsupported `run-in` and
  ruby roles are explicit deferrals rather than aliases for block.
- `FormattingContextKind`: block, inline, flex, grid, or table. This names the
  algorithm responsible for descendants; it is not a Taffy display value.
- `principal_box(node) -> Option<BoxId>` and
  `boxes_for_node(node) -> &[BoxId]`.
- `fragments(box) -> &[Fragment]`, with a DOM-facing compatibility lookup
  defined in terms of box provenance while callers migrate.
- provenance from every generated box and fragment back to the originating
  DOM node for paint, hit testing, accessibility geometry, and CSSOM.

The first structural fixtures cover:

1. one element with a principal box and several fragments;
2. one inline element split around an in-flow block, producing several boxes;
3. an anonymous table wrapper/row/cell whose origin remains traceable;
4. `display: none`, which has no principal box, versus `display: contents`,
   which has no principal box but whose children still generate boxes;
5. pseudo and replaced boxes, even where their later layout is still
   deferred.

## Stages, each with a receipt

- **B-1 - identity and provenance. COMPLETE 2026-07-26.** Land the contract
  above and its structural fixtures. No Taffy types appear in the public
  model. Receipt: the five contract fixtures plus the generated-tree fixture
  pass and current DOM-based fragment lookup remains unchanged.
- **B0 - the tree, with today's behaviour. COMPLETE 2026-07-26.** Build
  `CssBoxTree` and lower it to the same Taffy tree Livery builds now. No
  behaviour change; the receipt is an absolute before/after run over every
  F3b reftest directory, with zero moved files. Unit receipt:
  the existing `genet-livery` suite and the B-1 structural fixtures pass.
  Removal receipt: Taffy builders consume `BoxId`, and their source maps no
  longer encode DOM-node-to-Taffy-node as the layout identity.
- **B1 - box generation and anonymous fixup.** Outer/inner display roles and
  the anonymous-wrapper rules, including the block-flow whitespace rule the
  parent plan currently has to scope to flex and grid because the emulation
  depends on the wrong behaviour. Receipt: structural fixtures for mixed
  block/inline children, inline splitting, `display: contents`, list items,
  and table fixup; remove `block_containers_are_out_of_scope_for_now`; the
  named CSS2 anonymous-box and whitespace families improve or hold, with zero
  unexplained corpus regressions.
- **B2 - formatting contexts.** BFC/IFC roots, float containment, and
  margin collapsing derived from context rather than from ad-hoc checks.
  Receipt: named CSS2 float-containment and margin-collapse families, plus
  fixtures proving which box established each context; delete the superseded
  ancestor/display special cases; zero unexplained corpus regressions.
- **B3a - table fixup and cell grid.** Generate missing wrappers, preserve
  row/column roles, and distribute spans over an explicit cell grid.
- **B3b - table sizing.** Implement fixed and auto table sizing from CSS
  Tables, including column and column-group contributions. The backend may
  execute resulting track constraints; it does not choose the algorithm.
- **B3c - captions and border models.** Caption placement, separate borders,
  collapsed-border conflict resolution, and row-relative offsets. This
  retires the positioned-row flattening guard. The `partial` marker on
  `table-layout` is removed only when B3a-c close every limitation it names.
  Each B3 slice names its css-tables and CSS2 families, records absolute
  before/after file and subtest counts, removes its superseded workaround,
  and permits zero unexplained regressions.
- **B4 - intrinsic sizing.** Carry and resolve the content keywords, ending
  the quiet `min-content` = `max-content` = `auto` collapse. Receipt:
  css-sizing min/max/fit-content families and structural intrinsic-contribution
  fixtures, with zero unexplained corpus regressions.
- **B5 - positioning categories.** Real `fixed` and `sticky`, each with its
  own containing block. Receipt: named css-position fixed/sticky containing
  block and scroll families, removal of their Taffy aliases, and zero
  unexplained corpus regressions.
- **B6 - used values for CSSOM.** One resolution point, generated from the
  CSSOM resolved-value rules rather than a handwritten list of properties.
  Receipt: named CSSOM resolved-value testharness families, deletion of the
  handwritten dispatch, and zero unexplained corpus regressions.

## B-1/B0 receipt - 2026-07-26

The public model is Taffy-free. It carries `BoxId`, origin and provenance,
separate display axes, positioning category, formatting-context kind,
containing-block rule, principal and one-to-many node lookup, and one-to-many
box fragments. Both current Taffy passes consume `BoxId`. Their source maps
contain generated or suppressed-tree identity; DOM identity is recovered only
at the compatibility fragment edge.

`display: none` nodes and ignored DOM nodes are not admitted as CSS boxes.
B0 keeps a private lowering-only tree for two old observable behaviours:
suppressed subtrees still have the backend shape they had before, and comments
still split inline runs even though they generate no node. The initial corpus
run caught the distinction: omitting suppressed boundaries regressed two grid
files, and omitting comment boundaries regressed
`flexbox-baseline-multi-line-vert-002.html`. Both are now named B1 removals,
not accidental semantics in `CssBoxTree`.

A detached rebuild of baseline commit `145c76f4dd6`, against the same live
dependencies and GPU, passed that flex file. Its test/reference PNGs remain
under `Code/testing/genet/wpt-ledger/2026-07-26_boxtree_b0_control`; this
distinguished a real B0 regression from machine-state drift before the
comment-boundary fix.

`cargo test -p genet-livery`: 116 passed.

Exact reftest status diff, frozen fixed-table baseline
`Code/testing/genet/wpt-ledger/2026-07-26_fixedtable` against
`Code/testing/genet/wpt-ledger/2026-07-26_boxtree_b0_final`:

| directory | before pass | after pass | moved files |
|---|---:|---:|---:|
| css-backgrounds | 218 | 218 | 0 |
| css-borders | 28 | 28 | 0 |
| css-flexbox | 348 | 348 | 0 |
| css-grid | 433 | 433 | 0 |
| css-multicol | 105 | 105 | 0 |
| css-position | 40 | 40 | 0 |
| css-tables | 59 | 59 | 0 |
| css-writing-modes | 224 | 224 | 0 |
| CSS2 | 4,289 | 4,289 | 0 |
| **total** | **5,744** | **5,744** | **0** |

## Sequencing against the cutover

The frozen pre-tree code baseline is `145c76f4dd6`; the plan itself landed in
the docs-only commit `f5e03bebf67`. The baseline includes the explicitly
partial fixed-table sizing slice. Its all-nine-directory F3b receipt moved
zero files, so it is the last layout capability slice before B-1/B0.

This plan does not block F4. F4 may flip if its own incumbent-equivalence bar
is met, but that flip is not a conformance claim.

It does bound what F4 means. Passing F4 with the current lowering proves
Livery can replace Stylo, not that Livery implements CSS; the parent plan's
ledger split says the same thing from the other side.

**B-1, B0, and B1 block new layout and paint capability slices.** Grammar,
color, DOM, and harness work may continue because they do not bind new
semantics to the old layout shape. B1 lifts a restriction the parent plan
measured at 131 files, and every layout capability added before the tree
exists is written against a model that is about to change.

## Non-goals

- Replacing Taffy. It stays the layout backend for flex, grid, and block.
- A second layout engine. This is one layer, not a fork of the lane.
- Blocking the F4 flip on completion. F4 has its own bar and keeps it.

## Done condition

Every B-1 through B6 receipt above is checked in. Livery's layout input is a
box tree whose identity, shape, and provenance are derived from CSS rather
than from Taffy's capabilities; formatting-context algorithms retain their
own responsibilities; used values resolve after layout; the deferral
register's two compounding entries are closed; and `display`'s catalog entry
states the values it implements without the grammar overstating them.
