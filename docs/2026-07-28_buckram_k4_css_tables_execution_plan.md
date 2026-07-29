# Buckram K4 CSS tables execution plan

**Date:** 2026-07-28

**Status:** scoped; execution starts from the accepted K3 closure commit and
receipt

**Architectural authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

**Replaces for execution:** B3a through B3c in the absorbed
[Livery box-tree plan](2026-07-26_livery_box_tree_and_formatting_contexts_plan.md)

## Ruling

Buckram owns CSS table layout. A table is not a Taffy grid, and a table row is
not a Taffy flex container.

K4 builds a table wrapper, a table grid, row and column tracks, cells and
spans, sizing, row layout, captions, borders, and table-specific rendering as
Buckram models. Taffy may still lay out a flex or grid formatting context
inside a table cell. It does not choose table tracks, place cells, synthesize
rows, or return the table's fragments.

The current Grid/Flex lowering is a compatibility bridge. K4 may narrow that
bridge gate by gate, with a named counter and explicit deferral, but K4 does
not close until the bridge and its flattening guard are deleted.

## Standards authority

K4 uses three kinds of authority, in this order:

1. [CSS 2.1 chapter 17](https://www.w3.org/TR/CSS2/tables.html) for the stable
   table box model, anonymous fixup, fixed layout, row and cell constraints,
   captions, and the separate and collapsed border models.
2. The [HTML table model](https://html.spec.whatwg.org/multipage/tables.html)
   for document-language inputs such as `colspan`, `rowspan`, `span`, row
   groups, and HTML's grid-forming rules, plus the
   [HTML rendering defaults](https://html.spec.whatwg.org/multipage/rendering.html#tables-2)
   for UA-sheet behavior.
3. Current WPT and measured browser interoperability where CSS 2.1 explicitly
   leaves behavior undefined or permits more than one algorithm.

[CSS Tables Level 3](https://drafts.csswg.org/css-tables-3/) is a gap map and
an interoperability design input. Its 2 May 2026 draft labels itself "Not
Ready For Implementation." A K4 receipt must not present that draft as stable
normative authority. When its algorithm is adopted to match current engines,
the receipt records the WPT family and browser evidence that justified the
choice.

`genet-layout` remains a differential oracle only. Matching its output is not
acceptance evidence for a table rule.

## Starting state

K4 starts after K3 has committed and frozen its full acceptance receipt. The
accepted base is `2f1ae56968c` (`Complete Buckram K3 closure ratchet`). The
first K4 task records that commit and produces fresh `css/CSS2/tables` and
`css/css-tables` expectation maps before changing code.

The latest complete table orientation available while this plan was written
is K3l:

| Corpus | Pass | Fail | Skip | Error |
|---|---:|---:|---:|---:|
| `css/CSS2/tables` | 66 | 184 | 889 | 0 |
| `css/css-tables` | 50 | 80 | 198 | 0 |

These counts are orientation, not K4's baseline. The accepted K3 closure
receipt owns the actual starting state.

The live implementation has useful pieces, but not a table engine:

- Buckram already names table, row-group, row, cell, column-group, column,
  caption, header-group, and footer-group roles.
- Buckram's box generator performs a partial anonymous-table repair, but it
  does not yet generate and use a distinct wrapper and grid box according to
  the table model.
- Livery's `Display` vocabulary omits `inline-table`, header and footer groups,
  columns, and column groups. Its UA sheet collapses `thead`, `tbody`, and
  `tfoot` onto one role and omits `col` and `colgroup`.
- `border-collapse`, `border-spacing`, `caption-side`, and `empty-cells` are
  still catalogued as unimplemented.
- `genet-livery` walks the DOM through `table_cells`, flattens row groups and
  rows, and gives every cell a Taffy grid coordinate.
- `algorithm_kind` maps a table formatting context to `Grid` and a table row
  to `Flex`.
- `fixed_column_widths` correctly proves one CSS 2.1 fixed-layout subset:
  first-row cell widths on a definite-width table. It omits columns, spans,
  spacing, and automatic layout, and it lives on the wrong side of the
  Buckram boundary.
- `table_is_flattenable` preserves old nesting for positioned rows because
  flattening deletes the box that owns the offset.

The first-row fixed arithmetic is salvageable as a fixture. The DOM walk,
flattening, and Grid/Flex lowering are deletion targets.

## Target model

K4 adds a table-owned model under `components/buckram/src/table/`:

```rust
pub struct TableGrid {
    pub wrapper: BoxId,
    pub grid: BoxId,
    pub rows: Vec<TableTrack>,
    pub columns: Vec<TableTrack>,
    pub row_groups: Vec<TableTrackGroup>,
    pub column_groups: Vec<TableTrackGroup>,
    pub cells: Vec<TableCell>,
    pub captions: Vec<BoxId>,
}

pub struct TableCell {
    pub box_id: BoxId,
    pub row_start: usize,
    pub column_start: usize,
    pub row_span: usize,
    pub column_span: usize,
}

pub struct TableLayoutOutput {
    pub column_sizes: Vec<f32>,
    pub row_sizes: Vec<f32>,
    pub fragments: Vec<TableFragment>,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    pub borders: TableBorderGrid,
}
```

The exact storage can change. These invariants may not:

1. Wrapper, grid, row group, row, column group, column, cell, and caption are
   distinct roles with stable box provenance.
2. Cell placement is an explicit `(row_start, column_start, row_span,
   column_span)`, not a backend placement side effect.
3. The HTML adapter normalizes HTML attributes into table inputs. Buckram's
   algorithm does not read DOM nodes or HTML attribute strings.
4. Intrinsic cell and table sizes use Buckram's query contract.
5. Row, group, cell, caption, and wrapper fragments survive into the shared
   `FragmentTree`.
6. Border winners and table paint order are table-layout outputs associated
   with fragment identity, not a second anonymous rectangle plane.
7. Logical axes stay primary. Physical geometry is derived at the fragment
   edge.

## Live code map

| Seam | K4 responsibility |
|---|---|
| `components/buckram/src/box_tree.rs` | exact table fixup, wrapper/grid identity, table roles |
| `components/buckram/src/table/mod.rs` | table dispatcher, input and output contracts |
| `components/buckram/src/table/grid.rs` | row/column topology, groups, spans, missing slots |
| `components/buckram/src/table/sizing.rs` | fixed and automatic inline sizing, intrinsic table sizes |
| `components/buckram/src/table/rows.rs` | row height, rowspan distribution, cell alignment and baselines |
| `components/buckram/src/table/borders.rs` | spacing, collapsed-border winners, border geometry |
| `components/buckram/src/taffy_adapter.rs` | cell-subtree flex/grid calls only; table dispatch remains Buckram-owned |
| `components/genet-livery/src/box_tree.rs` | computed display roles and table style lowering |
| `components/genet-livery/src/layout.rs` | HTML table inputs, intrinsic provider, fragment integration |
| `components/genet-livery/src/paint.rs` | table background, cell, and resolved-border painting |
| `components/genet-livery/src/lib.rs` | HTML UA table defaults |
| `components/livery/{properties.toml,build.rs}` | table property grammar and computed values |

Start with `table.rs` if one file stays reviewable. Split it into the named
modules before unrelated topology, sizing, and border rules become one review
unit.

## Compatibility bridge

K4 introduces a temporary `TableDeferral` and a count of tables that still use
the old Grid/Flex bridge. A deferral names the missing CSS distinction, not a
DOM tag pattern or a Taffy limitation.

Every gate must:

- narrow at least one deferral or add a model needed to narrow it in the next
  gate;
- state the exact remaining bridge count in live fixtures;
- preserve the old path only for the named unsupported role;
- avoid claiming the bridge's WPT movement as table conformance; and
- delete obsolete variants as the model takes over.

K4 closure permits routed K5 positioning and K6 fragmentation gaps. It does
not permit a live table to fall back to the table-as-grid or row-as-flex
bridge.

## Execution order

| Gate | Outcome | Relative difficulty |
|---|---|---|
| K4a | complete table vocabulary and exact box generation | medium |
| K4b | standards-shaped row/column grid and HTML span adapter | high |
| K4c | fixed, automatic, and intrinsic inline sizing | very high |
| K4d | row layout, cell alignment, baselines, and dedicated table dispatch | very high |
| K4e | wrapper flow, inline-table, captions, and float avoidance | high |
| K4f | separated-border rendering, backgrounds, empty cells, and collapsed tracks | high |
| K4g | collapsed-border conflict resolution, geometry, and paint | very high |
| K4h | positioned-table seam and K4 closure audit | medium |

Accepted implementation gates land serially. Research for an upcoming gate can
run separately, but K4b through K4g repeatedly touch the same table model,
Livery adapter, fragments, and conformance baseline.

## K4a. Table vocabulary and box generation

### Outcome

Make the generated box tree capable of representing the complete CSS 2.1 and
HTML table structure before any algorithm lowering.

### Work

- Add computed display values for `inline-table`, `table-header-group`,
  `table-footer-group`, `table-column-group`, and `table-column`.
- Give `table` and `inline-table` separate outer roles with the same table
  inner role.
- Implement `border-collapse`, `border-spacing`, `caption-side`, and
  `empty-cells` parsing, inheritance, initial values, serialization, and
  computed storage.
- Bring the HTML UA table rules into line with the HTML rendering defaults,
  including distinct header/footer groups, columns, column groups,
  `border-spacing: 2px`, cell padding, and inherited vertical alignment.
- Generate explicit wrapper and grid boxes for a table-root and split the
  element's computed properties between them according to CSS 2.1.
- Replace the current partial repair with the ordered CSS anonymous-table
  fixup stages: irrelevant boxes, missing child wrappers, and missing parent
  wrappers.
- Preserve source and anonymous provenance through every repair.

### Evidence

- Pure box-tree fixtures cover each missing child and parent wrapper rule,
  whitespace removal, out-of-flow children, nested improper table boxes, and
  distinct wrapper/grid provenance.
- Livery fixtures cover all display keywords on arbitrary non-HTML elements
  and the HTML UA roles for `thead`, `tbody`, `tfoot`, `colgroup`, and `col`.
- Property tests cover valid, invalid, inherited, initial, and computed values
  for the four table properties.
- Focused WPT: CSS2 `table-anonymous-objects-*`, `html-display-table`,
  `row-group-order`, and table-property parsing/computed tests.

### Stop rules

- Stop if wrapper and grid must share one box identity.
- Stop if a missing wrapper is inferred from a backend display enum.
- Do not move cell placement or sizing into K4a.

### Removal receipt

Delete anonymous-table repair rules that contradict the complete ordered
fixup. The live Grid/Flex bridge remains, now fed by the corrected box model.

## K4b. Row/column grid and HTML span adapter

### Outcome

Build one explicit table grid from generated boxes and document-language span
inputs.

### Work

- Add `TableGrid`, row and column tracks, track groups, cells, captions, and
  slot occupancy.
- Preserve header and footer ordering rules without rewriting DOM order.
- Normalize HTML `colspan`, `rowspan`, `col span`, and `colgroup span` in the
  Livery adapter. Honor HTML's bounds and `rowspan="0"` downward-growth rule.
- Place each cell in the next unoccupied slot and carry both row and column
  spans. Record table-model overlaps as explicit input errors without losing
  deterministic layout.
- Create missing slots and explicit column tracks without inventing generated
  CSS cell boxes where only table-grid occupancy is required.
- Feed the compatibility bridge from `TableGrid` so topology has one owner.

### Evidence

- Pure fixtures cover simple rows, rowspan occupancy, colspan growth,
  `rowspan="0"`, overlapping malformed input, columns, column groups, multiple
  row groups, and header/footer ordering.
- Adapter fixtures prove HTML normalization and CSS-display tables that have
  no HTML span attributes.
- Live fixtures expose stable row, column, and cell placements before and
  after bridge layout.
- Focused WPT: `colspan-*`, `rowspan-*`, `table_grid_size_*`,
  `column-track-merging`, `row-group-order`, and HTML table-model cases.

### Stop rules

- Stop if Buckram must read HTML attributes.
- Stop if a span is expressed as a Taffy `GridPlacement`.
- Do not merge tracks merely because Taffy's implicit grid would merge them.

### Removal receipt

Delete `table_cells`. The temporary `place_table_cell` bridge consumes
`TableGrid` placements and is now the only remaining flattened-grid path.

## K4c. Fixed, automatic, and intrinsic inline sizing

**Execution plan:** [Buckram K4c table inline sizing execution
plan](2026-07-28_buckram_k4c_table_inline_sizing_execution_plan.md)

### Outcome

Make Buckram the sole owner of table and column inline sizes.

### Work

- Move the existing first-row fixed arithmetic into Buckram and retain its
  fixture.
- Complete fixed layout with column and column-group contributions, first-row
  spanning-cell distribution, border or spacing offsets, remaining-space
  distribution, table minimum width, and later-row overflow.
- Implement automatic layout through Buckram intrinsic queries: cell
  min-content and max-content contributions, single-column constraints,
  spanning-cell distribution, columns, column groups, percentages, table
  intrinsic widths, and used table width.
- Make `table-layout: fixed` with `width: auto` follow the selected CSS 2.1
  policy explicitly.
- Supply intrinsic table widths to block, float, inline-table, flex-item, and
  grid-item parents without reading final backend layout.
- For CSS 2.1's deliberately non-normative automatic algorithm, record the
  chosen interoperable distribution with exact WPT and current Chrome/Firefox
  evidence.
- Continue to use the bridge only as a placement consumer of Buckram's final
  column sizes.

### Evidence

- Pure fixtures exercise available widths below min-content, between intrinsic
  bounds, and above max-content; columns; column groups; percent columns;
  colspan distribution; spacing; both directions; and fixed versus auto.
- Adapter fixtures prove cell content is queried through Buckram and no table
  width is recovered from Taffy Grid.
- Live fixtures cover block table, inline-table, float, flex item, and grid
  item intrinsic sizing.
- Focused WPT: the complete `fixed-table-layout-*` family, with
  `fixed-table-layout-003a*` through `003c*` accepted here and the collapsed
  `003d*` through `003f*` cases classified for K4g,
  `table-intrinsic-size-*`, `table-as-item-*`, `min-max-size-table-*`,
  colspan sizing, and CSS sizing table cases.

### Stop rules

- Stop if automatic sizing reads completed cell or grid rectangles.
- Stop if a percentage cycle silently becomes zero or `auto`.
- Stop if the current CSS Tables 3 draft is the only evidence for an
  underspecified distribution choice.

### Removal receipt

Delete Livery's `fixed_column_widths` and table-specific `horizontal_edges`
helper. Narrow the `table-layout` partial marker after its column, span,
separated-spacing, and automatic-layout limitations close. K4g removes it
only after winning collapsed-border geometry feeds the same sizing algorithm.

## K4d. Row layout, cell alignment, and dedicated table dispatch

**Execution plan:** [Buckram K4d table row layout execution
plan](2026-07-28_buckram_k4d_table_row_layout_execution_plan.md)

### Outcome

Produce the table's used block size, baselines, and fragments through a
Buckram table algorithm, then retire Grid/Flex algorithm selection for tables.

### Work

- Add `AlgorithmKind::Table` or an equivalent Buckram-owned dispatch that does
  not enter Taffy's grid algorithm.
- Lay out cell contents at their resolved column widths.
- Compute minimum row heights from row styles, cell styles, content, spacing,
  and borders.
- Distribute rowspan requirements and definite extra table height.
- Treat percentage and cyclic block sizes as explicit outcomes. Record an
  interop decision where CSS 2.1 leaves distribution undefined.
- Compute cell and row baselines and apply table-cell `vertical-align`
  baseline, top, middle, and bottom rules.
- Emit wrapper-independent grid, row-group, row, column-group, column, and
  cell fragments with correct containing-fragment relationships.
- Preserve overflow from later fixed-layout rows without letting their content
  change column widths.

### Evidence

- Pure fixtures cover row minimums, rowspan distribution, definite extra
  height, baseline fallback, all four table-cell alignment modes, and
  indefinite percentage height.
- Adapter fixtures prove that flex and grid inside cells still call Taffy
  while the table itself does not.
- Live fixtures assert every internal fragment role, first and last baselines,
  containing fragments, overflow, and fixed versus automatic widths.
- Focused WPT: `height-distribution`, CSS2 table-height families,
  `baseline-vertical`, `table-vertical-align-*`, percent-height table-cell
  cases, flex/grid-in-cell cases, and the complete table directories.

### Stop rules

- Stop if rows must disappear to make the algorithm run.
- Stop if a baseline is recovered by walking Taffy descendants after the cell
  formatting context returns.
- Route fragmentation-dependent rowspan sizing to K6.

### Removal receipt

Delete `place_table_cell`, `table_is_flattenable`, table-to-Grid,
row-to-Flex, and all table-specific Taffy style mutations. Positioned table
parts remain boxes and fragments even before K4h applies their table-specific
offset seam.

## K4e. Wrapper flow, inline-table, captions, and float avoidance

### Outcome

Make the table wrapper the box that participates in normal flow while the
table grid owns tracks and cell geometry.

### Work

- Apply table margins, float, position category, and outer display to the
  wrapper; apply table grid properties to the grid box.
- Support block table and inline-table intrinsic and shrink-to-fit behavior.
- Reuse K3's block equations for auto margins and table-specific avoidance
  beside floats.
- Lay out top and bottom captions between wrapper margins and table borders,
  including multiple captions, caption margins, intrinsic contributions, and
  writing modes.
- Emit separate wrapper, grid, and caption fragments with stable provenance.
- Define CSSOM and hit-test selection of wrapper, grid, and caption geometry
  explicitly rather than choosing a principal rectangle by accident.

### Evidence

- Pure fixtures cover wrapper/grid property split, top and bottom captions,
  multiple captions, inline-table shrink-to-fit, auto margins, and a table
  that moves below a float.
- Live fixtures prove paint, hit testing, and used geometry address the
  intended fragments.
- Focused WPT: CSS2 `caption-*`, `caption-position-*`, `anonymous-table-box-width`,
  table margins, inline-table, floats around tables, writing-mode captions,
  and table CSSOM geometry cases.

### Stop rules

- Stop if captions are inserted as grid rows.
- Stop if the table grid's margin box is used as the wrapper.
- Keep fragmentation and repeated headers in K6.

### Removal receipt

Delete K3's table-specific float-avoidance route and any wrapper/grid
principal-rectangle compatibility choice.

## K4f. Separated borders, backgrounds, empty cells, and collapsed tracks

### Outcome

Complete the separated-border model and table-specific rendering order.

### Work

- Consume the horizontal and vertical `border-spacing` already accounted for
  by K4c and K4d when painting table gaps. Do not add it to track or intrinsic
  geometry a second time.
- Implement table, column-group, column, row-group, row, and cell background
  layers through table fragments.
- Paint cells in DOM order even when span placement changes their grid
  position.
- Implement `empty-cells: show | hide` from actual in-flow and floated cell
  content.
- Implement `visibility: collapse` for rows, row groups, columns, and column
  groups by supplying a track-visibility mask to the accepted K4c and K4d
  algorithms while retaining the constraints required by the table model.
- Account for table box shadow, overflow, and background clipping at the
  wrapper/grid boundary.

### Evidence

- Pure paint-order fixtures cover every table background layer, spacing,
  spanning cells, DOM-order paint, empty cells, and collapsed tracks.
- Live image fixtures distinguish table, group, row, column, and cell colors.
- Focused WPT: CSS2 separated-border, table-background, empty-cell,
  row/column visibility, `whitespace-001`, and css-tables tentative paint
  families.

### Stop rules

- Stop if row or column paint depends on a rectangle reconstructed from cells.
- Stop if `visibility: collapse` deletes sizing inputs.
- Do not share collapsed-border winners with the separated model.

### Removal receipt

Delete generic block paint assumptions for table-internal fragments where
table paint order overrides them.

## K4g. Collapsed-border conflict resolution and paint

**Execution plan:** [Buckram K4g collapsed border execution
plan](2026-07-28_buckram_k4g_collapsed_border_execution_plan.md)

### Outcome

Compute one resolved border grid before sizing and paint it in the correct
table phase.

### Work

- Resolve every cell edge across table, column group, column, row group, row,
  and cell candidates.
- Apply CSS 2.1 precedence for `hidden`, `none`, width, style, originating
  role, direction, and source order.
- Handle spans, half-border intrinsic offsets, table outer edges, corners,
  odd-device-pixel rounding, and overflow.
- Re-run fixed and automatic sizing with collapsed-border offsets rather than
  separated spacing.
- Paint resolved borders once, in the table border phase, with the required
  spanning-cell and collapsed-border order.

### Evidence

- Pure fixtures exercise each conflict tiebreak, LTR and RTL source order,
  spans, table edges, odd widths, and border-style conversions.
- Live fixtures compare separate and collapse modes with identical content and
  prove that winning borders affect both geometry and paint.
- Focused WPT: CSS2 `border-conflict-*`, `collapsing-border-model-*`,
  `border-collapse-*`, css-tables collapsed-border geometry, subpixel cases,
  spanning-cell cases, the collapsed `fixed-table-layout-003d*` through
  `003f*` families, and the tentative collapsed-border paint-order family.

### Stop rules

- Stop if conflict resolution happens during paint.
- Stop if a collapsed border's width is absent from intrinsic sizing.
- Stop if DOM order alone is used where CSS conflict precedence applies.

### Removal receipt

Delete collapsed-border generic box painting and every fallback that treats
`border-collapse: collapse` as separated borders with zero spacing.
Remove the `table-layout` partial marker after the accepted collapsed-border
metrics feed K4c's fixed and automatic sizing algorithms.

## K4h. Positioned-table seam and K4 closure

### Outcome

Remove the remaining positioned-table compatibility seams, prove every
surviving gap is owned, and close K4.

### Work

- Apply relative offsets to preserved row-group, row, and cell fragments. The
  old "cells owed a row-relative shift" shape is unnecessary because the
  owning boxes now survive.
- Expose correct table wrapper and internal containing fragments for K5's
  absolute, fixed, sticky, and static-position algorithms.
- Inventory every `TableDeferral`, table-specific compatibility flag, and
  remaining Taffy dispatch.
- Route absolute/fixed/sticky positioning to K5 and table fragmentation,
  repeated headers, and split rowspans to K6.
- Prove the Grid/Flex bridge deleted by K4d has not been reintroduced.
- Delete positioned-table compatibility counters, diagnostic switches, and
  every obsolete deferral.
- Append the final K4 receipt to the architecture plan and replace its K4
  paragraph with the exact K5 and K6 routes.

### Closure evidence

- Every live table uses Buckram table dispatch.
- No Taffy type appears in the public table model or table output.
- Wrapper, grid, captions, tracks, groups, rows, columns, and cells retain box
  and fragment identity.
- Fixed and automatic inline sizing use Buckram intrinsic queries.
- Separate and collapsed border geometry are distinct and affect sizing.
- Paint, hit testing, accessibility geometry, and CSSOM consume table
  fragments rather than a flattened cell plane.
- The `fixed-table-layout-003*`,
  `table-anonymous-objects-*`, caption, height/baseline, separated-border, and
  collapsed-border families have exact before/after receipts.
- Complete `css/CSS2`, `css/css-tables`, `css/css-writing-modes`,
  `css/css-position`, and all-nine comparisons have zero unexplained
  regressions.

K4 closure does not claim table fragmentation or complete positioning.
Those gaps are named K6 and K5 work, and the table remains on Buckram's engine
path while they are open.

## Acceptance ladder for every gate

1. **Model proof:** pure Buckram fixtures name the CSS distinction.
2. **Adapter proof:** HTML and Livery values are normalized into Buckram
   inputs without DOM or Taffy types crossing the algorithm boundary.
3. **Live proof:** generated boxes, fragments, baselines, paint data, and
   dispatch counters show the same behavior through Livery.
4. **Property proof:** parsing, cascade, computed values, and serialization
   are tested when a gate adds table vocabulary.
5. **Focused corpus:** fresh reftest and testharness results for the named
   family.
6. **Regression ratchet:** exact status maps against the prior accepted gate.
   Run complete CSS2 whenever a CSS2 table family moves. Compare all nine when
   shared flow, sizing, or paint code moves.
7. **Interop receipt:** behavior left open by CSS 2.1 records current WPT and
   browser evidence before implementation.
8. **Build proof:**

   ```powershell
   cargo test -p buckram -p livery -p genet-livery --offline
   cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings
   rustfmt --edition 2024 --check <touched Rust files>
   git diff --check
   cargo build -p genet-wpt --release --all-features --offline
   ```

Generated WPT expectations, screenshots, and logs remain outside Git.

## Working-tree and commit discipline

- Start from the accepted K3 closure commit.
- Record unrelated dirty paths before every gate and preserve them.
- One gate produces one reviewable commit and one receipt.
- Stage only the gate's files and this plan's receipt.
- Failed broad admissions are reverted or narrowed before commit. Keep the
  boundary evidence in the receipt.
- Do not rewrite the accepted K3 closure baseline.

## First executable task

The K4a handoff is:

> Read this plan, the architecture plan's accepted K3 closure receipt, CSS 2.1
> sections 17.2 through 17.4, and the HTML table rendering defaults. Freeze
> fresh `css/CSS2/tables` and `css/css-tables` maps at the accepted K3 commit.
> Execute K4a only: complete table display and property vocabulary, generate
> distinct wrapper and grid boxes, and implement the ordered anonymous-table
> fixup. Preserve the current live layout bridge. Stop after the model,
> adapter, property, focused-corpus, and build receipts pass. Append the K4a
> receipt, stage only K4a paths, and commit.

K4a is the right first slice because it corrects the input model without
mixing in track sizing. K4b can then build one table grid from stable box
roles instead of compensating for missing structure inside an algorithm.
