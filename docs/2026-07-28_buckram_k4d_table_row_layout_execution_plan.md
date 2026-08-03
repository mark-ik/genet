# Buckram K4d table row layout execution plan

**Date:** 2026-07-28

**Status:** in execution from accepted K4c5b commit `a96fe7d147e`. K4d1
through K4d5 are complete, and K4d6 is split: K4d6a's fragment model has
landed. Both 10/10 gates selected their algorithms from measured Chrome 150 /
Firefox 153 matrices. K4d6b is next: it is the live cutover and the
bridge-deletion gate, so it is the first K4d gate whose WPT maps are expected
to move.

**Parent plan:** [Buckram K4 CSS tables execution plan](2026-07-28_buckram_k4_css_tables_execution_plan.md)

**Predecessor plan:** [Buckram K4c table inline sizing execution plan](2026-07-28_buckram_k4c_table_inline_sizing_execution_plan.md)

**Architectural authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

## Ruling

K4d makes Buckram the sole owner of table block-axis layout.

The table algorithm consumes K4b's `TableGrid`, K4c's final column sizes, the
table's computed block-size constraints, and a callback that formats each
cell's contents at a known inline size. It returns row sizes, cell placement
and alignment, table baselines, overflow, and fragment drafts for every
table-internal box.

Taffy may format a flex or grid formatting context inside a cell. It does not
represent a table row as flex, represent the table as grid, distribute row
height, select a cell baseline, or produce table-internal fragments.

K4d is the retirement gate for the table-to-Grid and row-to-Flex bridge. K4h
later audits that the bridge remains absent while it closes relative
positioning and routes the remaining positioned and fragmented cases.

## Entry gate

Execution starts from an accepted K4c commit and receipt. It does not start
from an in-progress K4a, K4b, or K4c tree.

The accepted predecessor must provide:

- K4a wrapper, grid, group, row, column, cell, and caption roles;
- K4b row and column topology with normalized row and column spans;
- one stable `BoxId` for every table box that can produce a fragment;
- K4c intrinsic table sizes and one final logical inline size per column;
- separated-mode inline border metrics; and
- explicit `CaptionMinPendingK4e`, `TrackVisibilityPendingK4f`, and
  `CollapsedBorderMetricsPendingK4g` outcomes.

K4d freezes its exact predecessor commit and produces fresh expectation maps
for:

- `css/CSS2/tables`;
- `css/css-tables`; and
- any other K3 ratchet directory moved by the accepted K4c implementation.

## Standards boundary

[CSS 2.1 section 17.5.3](https://www.w3.org/TR/CSS2/tables.html#height-layout)
defines several stable requirements:

- an automatic table height is the sum of row heights plus applicable
  spacing or borders;
- a definite table height is a minimum;
- a row's minimum height is the maximum of its specified height,
  single-row cell height constraints, and the minimum required by its cells;
- a cell's specified height influences the row but does not directly enlarge
  the cell's content box; and
- a rowspan must collectively fit within the rows it spans.

CSS 2.1 deliberately leaves these decisions undefined:

- distribution of table height above the sum of row minima;
- percentage heights on table rows and cells;
- the meaning of height on row groups; and
- the distribution of a spanning cell's minimum over multiple rows.

K4d does not disguise those gaps as straightforward specification work.
Every selected rule needs current WPT and browser evidence.

CSS 2.1 defines the table-cell baseline and the ordered `vertical-align`
procedure. A cell baseline is the first in-flow line-box baseline or first
in-flow table-row baseline, whichever comes first, with the bottom content
edge as fallback. Baseline-aligned cells establish the row baseline before
top, bottom, and middle cells are finally positioned.

[CSS 2.1 section 10.8](https://www.w3.org/TR/CSS2/visudet.html#line-height)
defines the baseline of an inline table as the baseline of its first row.
[CSS Box Alignment Level 3](https://drafts.csswg.org/css-align-3/#baseline-export)
provides the current first and last baseline-set model for table boxes and
rows. K4d records which of those newer rules its `Baselines` output adopts.

The current [CSS Tables Level 3 draft](https://drafts.csswg.org/css-tables-3/#height-distribution)
is an interoperability design input. Its height section contains editorial
TODOs and browser-divergence notes. Its two-pass structure, span-ordering
shape, and distribution rules may be selected only when current evidence
supports them.

For each undefined decision, append an interop record containing:

- the exact WPT or reduced fixture;
- current Chrome and Firefox build versions;
- table, row, cell, content, and overflow block sizes observed;
- the competing algorithms considered; and
- the rule accepted for Buckram.

## Live debt at the entry seam

The current algorithm boundary already has useful standards-owned pieces:

- `AlgorithmTree` owns source identity and calls Taffy privately.
- `AlgorithmKind` distinguishes leaf, block, flex, and grid algorithms.
- `Baselines` stores finite logical offsets.
- formatting contexts can declare baselines before parent propagation.
- `FragmentTree` stores one-to-many fragments by `BoxId`, with structural
  parent and containing-fragment relationships.

The table path still violates the target boundary:

- `algorithm_kind` maps a table formatting context to `AlgorithmKind::Grid`.
- It maps a table row to `AlgorithmKind::Flex`.
- `anonymous_taffy_style` repeats the same table-to-Grid and row-to-Flex
  choice.
- `place_table_cell` expresses table placement as Taffy grid placement.
- `table_is_flattenable` removes rows unless a positioned row exposes why
  that removal is incorrect.
- table fragments are recovered from the flattened backend layout rather than
  emitted by a table algorithm.
- generic baseline propagation does not know the table-cell and row rules.

K4d removes those decisions instead of adding another correction pass over
their rectangles.

## Row-layout contracts

The exact Rust spelling may change during K4d1. These ownership boundaries may
not.

```rust
pub enum TableCellLayoutPass {
    Measure,
    ResolvePercentages { cell_block_size: f32 },
}

pub struct TableCellLayoutInput {
    pub box_id: BoxId,
    pub content_inline_size: f32,
    pub available_block_size: Option<f32>,
    pub percentage_basis: Option<f32>,
    pub pass: TableCellLayoutPass,
}

pub struct TableCellLayoutOutput {
    pub content_block_size: f32,
    pub border_box_min_block_size: f32,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    pub fragments: FragmentDraftTree,
}

pub struct TableRowMeasure {
    pub row: BoxId,
    pub min_block_size: f32,
    pub preferred: TableBlockConstraint,
    pub constrained: bool,
}

pub struct TableBlockSizingInput<'a> {
    pub grid: &'a TableGrid,
    pub inline: &'a TableInlineSizingResult,
    pub table_constraint: TableBlockConstraint,
    pub border_metrics: TableBlockBorderMetrics,
    pub available_block_size: Option<f32>,
    pub track_visibility: TableTrackVisibility,
}

pub struct TableRowLayoutResult {
    pub used_table_block_size: f32,
    pub row_offsets: Vec<f32>,
    pub row_sizes: Vec<f32>,
    pub cells: Vec<TableCellPlacement>,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    pub fragments: FragmentDraftTree,
}
```

The contracts preserve these distinctions:

1. A cell's content size, specified height contribution, border-box minimum,
   final span rectangle, and content alignment offset are separate values.
2. First-pass fragments remain drafts. A second percentage pass replaces
   them rather than inserting a duplicate subtree into `FragmentTree`.
3. A formatting context returns its baselines directly. The table algorithm
   does not rediscover them by walking Taffy descendants.
4. Percentage constraints keep an explicit basis or an explicit indefinite
   result.
5. Separated vertical spacing and collapsed winning-border metrics are
   different `TableBlockBorderMetrics` variants.
6. `TableTrackVisibility` defaults every row to visible and can later collapse
   tracks without deleting their pre-collapse constraints.
7. Row offsets and sizes use the table's logical block axis. Physical
   coordinates are derived only when final fragments are committed.
8. `row_offsets.len() == row_sizes.len() == TableGrid::rows.len()`.
9. Every final cell rectangle covers exactly its normalized row and column
   span.

Expected table-specific deferrals include:

- `PercentageBlockBasisIndefinite`;
- `PercentageBlockCycle`;
- `FragmentationDependentRowspan`;
- `CollapsedBlockBorderMetricsPendingK4g`; and
- the K5 positioned-content outcomes already owned outside K4d.

An undefined percentage does not silently become zero or `auto`. A measured
interop rule may deliberately treat it as automatic in a named pass, but the
reason remains visible in the receipt.

## Dispatch seam

`AlgorithmKind::Table` is the expected dispatch tag, but the table algorithm
does not become a Taffy algorithm.

The accepted shape is:

1. the algorithm adapter recognizes a Buckram table formatting context;
2. it converts generic available-space inputs to Buckram logical inputs;
3. `buckram::table` performs table sizing and placement using Buckram types;
4. a private cell-format callback invokes the appropriate leaf, block, flex,
   grid, or inline formatting context for cell contents; and
5. the adapter converts the final table result back to the scratch-tree
   placement needed by its caller.

`components/buckram/src/table/` must not import Taffy types. If adding
`AlgorithmKind::Table` would force `LayoutInput`, `LayoutOutput`, `NodeId`,
`Style`, or backend cache rules into that module, use a Buckram-owned
dispatcher immediately outside the Taffy adapter instead.

## Execution gates

| Gate | Outcome | Difficulty |
|---|---|---:|
| K4d1 | table dispatch and cell-formatting contracts | 8/10 |
| K4d2 | first-pass cell layout and single-span row minima | 8/10 |
| K4d3 | rowspan constraints and used table height | 10/10 |
| K4d4 | percentage-height relayout and cycle handling | 10/10 |
| K4d5 | cell alignment and table baseline sets | 9/10 |
| K4d6 | fragment emission, live dispatch, and bridge deletion | 9/10 |

Accepted implementation gates land serially. The K4d3 and K4d4 browser
matrices may be collected while K4d1 and K4d2 execute. They do not select an
algorithm until their evidence is attached to the accepting receipt.

One task owns one gate, appends its receipt, stages only its paths, and
commits. It does not begin the next gate.

## K4d1. Table dispatch and cell-formatting contracts

### Outcome

Create a Buckram table algorithm boundary that can format cell contents
without exposing table layout to Taffy.

### Work

- Add `components/buckram/src/table/mod.rs` and
  `components/buckram/src/table/rows.rs` if K4b and K4c have not already
  created the module split.
- Define the row-layout inputs, cell-format callback, fragment drafts,
  results, metrics, constraints, and deferrals described above.
- Accept an all-visible `TableTrackVisibility` input that K4f can later
  replace without forking row layout.
- Add `AlgorithmKind::Table` or the equivalent Buckram-owned dispatch tag.
- Convert generic available space to logical table inputs at the adapter
  edge.
- Give the cell-format callback the exact content inline size derived from
  K4c's spanned columns and inline offsets.
- Allow the callback to format block, inline, flex, and grid contents and
  return direct baselines and overflow.
- Add a shadow table-dispatch fixture. Do not switch live tables away from
  the compatibility bridge in this gate.

### Evidence

- Pure fixtures construct a table input without DOM or Taffy types.
- Adapter fixtures format block, retained-inline, flex, and grid cell
  contents at an exact inline size.
- A flex or grid inside a cell records one backend call, while the table and
  row record none.
- Fragment-draft fixtures prove a discarded measurement pass cannot leak
  fragments into the final tree.
- The fresh K4c expectation maps are copied into the K4d proof directory as
  the unchanged baseline.

### Stop rules

- Stop if `buckram::table` imports a Taffy type.
- Stop if a cell is identified by DOM node rather than `BoxId`.
- Stop if a callback returns only a rectangle and loses baselines, overflow,
  or child fragment drafts.
- Stop if the shadow dispatcher mutates live layout.

### Removal receipt

No live bridge code is deleted in K4d1. Record the exact dispatch and callback
types that will replace it.

### K4d1 receipt - 2026-08-01

**Base commit:** `a96fe7d147e` (accepted K4c5b).

**Capability:** `components/buckram/src/table/rows.rs` defines the row-layout
boundary: `TableBlockConstraint`, `TableSeparatedBlockMetrics` and
`TableBlockBorderMetrics` (block padding pre-resolved, since CSS resolves
padding percentages against the inline containing size that K4c made real),
`TableBlockDeferral` with the four named deferrals, `TableCellLayoutPass`,
`TableCellLayoutInput`/`Output`, the `TableCellFormatter` trait,
`TableRowMeasure`, `TableBlockSizingInput`, `TableCellPlacement`,
`TableRowLayoutResult`, and `FragmentDraftTree`. Drafts are deliberately not
`Fragment`s: no commit path into `FragmentTree` exists until K4d6, so a
discarded pass cannot leak into painted output by construction, and a parent
draft must precede its children. `spanned_cell_content_inline_size` derives
each cell's exact content inline size from K4c's spanned columns plus crossed
spacing minus resolved offsets; `format_table_cells` is the K4d1 dispatch
skeleton, first pass only, collapsed metrics deferring by name.
`AlgorithmKind::Table` is reserved at the adapter; nothing constructs it
until K4d6, and `buckram::table` imports no Taffy type.

**Pure fixtures:** exact single-span and spanned inline sizes (including the
crossed spacing interval), collapsed-metrics deferral, out-of-bounds span
error, draft-tree parent ordering, and discarded-pass draft dropping. Four
tests; buckram 127 total.

**Adapter fixture:** `k4d1_cell_formatter_formats_contents_at_exact_inline_sizes`
implements `TableCellFormatter` over the live `AlgorithmTree`, formats a
block cell and a flex cell at the exact K4c column sizes, and proves the
scratch tree contains only cell subtrees: no node represents the table or
row, and no table-as-Grid node exists in the dispatch shape.

**WPT:** the K4c5b maps are copied into the K4d proof directory as the
baseline, and both table corpora rerun with zero movement, as a model-only
gate requires. Proof directory:
`testing/genet/wpt-ledger/2026-08-01_buckram_k4d1`.

**Verification:** buckram 127, livery and genet-livery all targets 0 failed;
clippy clean on touched files (one pre-existing `is_multiple_of` warning in
unrelated adapter tests); exact-file Rustfmt and `git diff --check` clean.

## K4d2. First-pass cell layout and single-span row minima

### Outcome

Compute content-based minimum row heights for cells that occupy one row.

### Work

- Format every originating cell at its final K4c inline size with an
  indefinite first-pass block size.
- Derive the content-required border-box block size from the returned content,
  padding, and the active separated border metrics.
- For each row, take the maximum of:
  - its applicable definite height;
  - definite height contributions from cells spanning only that row; and
  - the minimum block size required by those cells.
- Keep the specified cell height as a row constraint. Do not overwrite the
  measured content-box height with it.
- Include separated vertical spacing exactly once at the table level.
- Handle empty rows, missing cells, empty cells, and rows containing only a
  continuing rowspan without inventing a flex item.
- Preserve content overflow separately from the row minimum.
- Keep fixed-layout later-row overflow from feeding back into K4c column
  sizes.

### Evidence

- Pure fixtures cover zero, one, and many cells; differing cell heights;
  row-versus-cell height constraints; padding; borders; empty rows; missing
  cells; vertical spacing; and subpixel values.
- Cell-order permutations within a row do not change the row minimum.
- Adapter fixtures prove cells receive exact content inline sizes and
  indefinite first-pass block sizes.
- Focused WPT includes `height-distribution/computing-row-measure-*`,
  `empty-table-height`, `dynamic-table-cell-height`,
  `subpixel-table-cell-height-*`, and simple CSS2 table-height cases.

### Stop rules

- Stop if a row enters `AlgorithmKind::Flex`.
- Stop if the largest completed cell rectangle is used without preserving
  the specified-height and content-minimum inputs that produced it.
- Stop if vertical spacing is attached to every cell.
- Stop if a cell height directly stretches its content formatting context.

### Removal receipt

Delete any shadow row-minimum arithmetic outside `buckram::table::rows`.
Live Grid/Flex placement remains until K4d6.

### K4d2 receipt - 2026-08-01

**Base commit:** `3ea63852115` (accepted K4d1).

**Capability:** `measure_single_span_rows` computes each K4b row's minimum as
the maximum of its own definite height, the definite specified-height
contributions of cells occupying only that row, and the border-box minimum
those cells' contents require. Supporting contracts: `CellBlockOffsets`
(resolved, since a padding percentage resolves against the inline containing
size K4c made definite) and `TableCellBlockStyle`, which keeps a cell's
specified height apart from its measured content so the specification
constrains the row without overwriting the content box.

**Boundary retained:** cells spanning more than one row contribute nothing;
distributing a spanning minimum is K4d3's undefined-in-CSS-2.1 decision.
Separated spacing is excluded from every row and belongs to the table level
exactly once, via `undistributable_block_size`. Overflow is retained on the
cell output and never inflates a row. `input.inline` is read-only, so
later-row content cannot feed back into K4c's columns. A percentage height
is never sampled at zero: it contributes nothing definite, leaves
`constrained` false, and survives in `preferred` for K4d4.

**Contract correction:** `TableRowMeasure::row` became `Option<BoxId>`. A row
track created implicitly by placement has no CSS box, and inventing an
identity for it would make a later fragment attributable to a box that does
not exist.

**Pure fixtures:** row maximum over differing cell heights with padding and
borders; cell-order permutation invariance; specified cell height as a row
constraint in both box-sizing modes; row-versus-cell competition; percentage
survival; empty rows, displaced cells, and rows holding only a continuing
rowspan inventing nothing; overflow and spacing exclusion; subpixel exactness;
and misaligned input vectors as explicit errors. Seven tests; buckram 134.

**Adapter fixture:**
`k4d2_row_minima_follow_from_formatted_cell_contents` formats real cell
contents over the live `AlgorithmTree` at the exact K4c inline size with an
indefinite first-pass block size, then derives row minima of 27px and 9px
from the measured contents alone.

**Removal:** an audit of `components/genet-livery/src` finds no row-height
arithmetic outside `buckram::table::rows`; only the new test's name matches.
Live Grid/Flex placement remains until K4d6.

**WPT:** both table corpora rerun against the K4d1 maps with zero movement,
as a model-only gate requires. The focused height families are captured as
orientation for K4d6's cutover, measured before any live change:
`height-distribution` 2 passed / 10 failed / 5 skipped of 17,
`empty-table-height` and `subpixel-table-cell-height-001` passing,
`dynamic-table-cell-height` skipped as script-dependent. Proof directory:
`testing/genet/wpt-ledger/2026-08-01_buckram_k4d2`.

**Verification:** buckram 134, livery and genet-livery all targets 0 failed;
clippy clean on touched files; exact-file Rustfmt and `git diff --check`
clean.

## K4d3. Rowspan constraints and used table height

### Outcome

Make every spanning cell fit its row range and choose the table's used block
size.

### Work

- Apply single-row minima before multi-row constraints.
- Process spanning-cell minimums in increasing span order.
- Ensure the sum of spanned row sizes and intervening spacing or borders is
  at least the cell's required block size.
- Select a deterministic distribution for span excess using the accepted
  Chrome, Firefox, and WPT matrix.
- Respect K4b's normalized row-group boundary and clamped rowspan decisions.
- For `height: auto`, use the row sum plus the active border-model space.
- Treat a definite table height as a minimum.
- Select and record an interoperable distribution for table height above row
  minima.
- Keep row-group height behavior explicit. CSS 2.1 leaves it undefined and
  the current CSS Tables draft ignores it.
- Preserve fragmentation-dependent spanning cases as K6 deferrals.

### Evidence

- Pure fixtures cover span two and greater; nested spans; spans beginning in
  different rows; spans at group boundaries; insufficient and excess table
  height; empty rows; and mixed constrained and automatic rows.
- The row sum plus border-model space equals the used table block size.
- Increasing one spanning-cell minimum never decreases a row in its span.
- Focused WPT includes rowspans, `computing-row-measure-*`,
  `extra-height-given-to-all-row-groups-*`, `min-height-table*`,
  `max-height-table`, and CSS2 table-height families.
- The receipt includes the exact distribution matrix for span excess,
  definite extra table height, empty rows, and row-group height.

### Stop rules

- Stop if a spanning cell is processed before the lower-span row measures
  under it.
- Stop if a definite table height is treated as an exact maximum.
- Stop if draft prose is the only reason for a distribution choice.
- Stop if a rowspan is shortened or moved after K4b topology acceptance.
- Route a row split across fragmentainers to K6.

### Removal receipt

Delete any adapter-side rowspan or table-height distribution. Buckram's row
result is the only source for used row and table block sizes.

### K4d3 interop matrix - 2026-08-01

Measured with `interop-matrix.html` in the K4d3 proof directory, headless:
**Chrome 150.0.0.0** (`--dump-dom`) and **Firefox 153.0.1** (`--screenshot`).
Each case reports every row's border-box block size and the table's.

| Case | Chrome 150 | Firefox 153 |
|---|---|---|
| S1 span 2, minima 20/40, cell 200 | 66.66 / 133.34 | 66.67 / 133.33 |
| S2 span 2, definite 20 + auto 40, cell 200 | 20 / 180 | 20 / 180 |
| S3 span 3, minima 10/20/30, cell 200 | 33.33 / 66.66 / 100.02 | 33.33 / 66.67 / 100 |
| S4 span 2, both rows empty, cell 200 | 0 / 200 | 0 / 200 |
| S5 span 2, both rows definite 20/30, cell 200 | 80 / 120 | 80 / 120 |
| T1 table 300, minima 20/40 | 100 / 200 | 100 / 200 |
| T2 table 300, minima 60/0 | 300 / 0 | 300 / 0 |
| T3 table 10, minima 20/40 | 20 / 40 (table 60) | 20 / 40 (table 60) |
| T4 row-group 200, minima 20/40 | 66.66 / 133.34 | 66.67 / 133.33 |
| T5 table 300, definite 20 + auto 40 | 20 / 280 | 20 / 280 |
| T6 table 300, two empty rows | 150 / 150 | 150 / 150 |
| **T7 table 300, both rows definite 20/30** | **120 / 180** | **145 / 155** |

**Eleven of twelve cases agree.** The accepted rule, reproduced by
`distribute_over_rows`:

1. Rows without a definite specified height absorb growth; rows with one keep
   it (S2, T5).
2. Growth is distributed in proportion to the rows' current sizes (S1, S3,
   T1, T2, T4).
3. When every row in scope is constrained, they all participate in proportion
   instead (S5, T7).
4. When every eligible weight is zero the two sites differ, and both branches
   are measured, not assumed: a rowspan gives everything to the last spanned
   row (S4), while table height splits equally (T6).
5. A definite table block size is a minimum, never a maximum (T3).

**The one divergence, T7,** is resolved toward Chrome. Firefox is internally
inconsistent there: it distributes a rowspan's excess proportionally over
all-definite rows (S5) but splits a table's excess equally over the same
shape (T7). Chrome uses one rule at both sites, and so does Buckram, which
keeps a single distribution function rather than two that differ only in a
rarely-reached branch. This is a measured tiebreak, not a reading of draft
prose.

**Row-group height (T4) is explicit but unimplemented.** Both engines apply a
row-group height exactly as a table height over that group's rows. Buckram
does not yet act on it because `TableBlockSizingInput` carries no per-group
constraint; adding one the adapter does not supply would be worse than the
recorded gap. K4d6 wires it when the adapter lowers group styles.

### K4d3 receipt - 2026-08-01

**Base commit:** `568959cef94` (accepted K4d2).

**Capability:** `size_table_rows` fits every spanning cell within its row
range and selects the table's used block size. Spanning cells are processed
in increasing span order, so a wider span always sees the rows a narrower one
already grew. Spacing a span crosses counts toward that cell's range, so only
the row sizes make up the remainder, and the table's undistributable total is
added exactly once. Returns `TableRowSizing` with row offsets in the table's
logical block axis.

**Interop decision:** the matrix above. All twelve cases are reproduced by
`rowspan_excess_follows_the_measured_interop_matrix` and
`used_table_block_size_follows_the_measured_interop_matrix`, including T7 at
Chrome's value.

**Pure fixtures:** the twelve matrix cases; spacing counted once at the table
and once per crossed interval inside a span; the monotonicity property that
raising a spanning requirement never shrinks a spanned row, checked across
five increasing requirements; and collapsed metrics deferring before any row
sizing. Five tests; buckram 139.

**Deferrals:** a rowspan reaching past the last row returns
`FragmentationDependentRowspan`. K4b already clamps spans to their row group,
so this is reachable only from a malformed grid; no fragmentainer exists in
this gate, and K6 owns the real split case.

**Removal:** no adapter-side rowspan or table-height distribution exists;
`size_table_rows` is the only source of used row and table block sizes.

**WPT:** both table corpora rerun against the K4d1 maps with zero movement,
as a model-only gate requires. Proof directory:
`testing/genet/wpt-ledger/2026-08-01_buckram_k4d3`, holding
`interop-matrix.html`, `chrome-dom.txt`, and `firefox.png`.

**Verification:** buckram 139, livery and genet-livery all targets 0 failed;
clippy clean on touched files; exact-file Rustfmt and `git diff --check`
clean.

## K4d4. Percentage-height relayout and cycle handling

### Outcome

Resolve percentage-dependent row, cell, and cell-descendant heights only when
a valid table or cell basis exists.

### Work

- Define the first-pass policy for percentage row and cell heights from the
  accepted interop matrix.
- After a definite used table height exists, perform the minimum additional
  row pass required to apply accepted percentage row and cell constraints.
- After final row distribution, relayout cell contents only when descendant
  percentage heights gained a definite cell basis.
- Distinguish replaced descendants, scroll containers, and visible, hidden,
  clip, auto, and scroll overflow where current engines do.
- Bound relayout to named passes. A dependency cycle returns
  `PercentageBlockCycle` rather than iterating until values appear stable.
- Replace first-pass fragment drafts and overflow with final-pass outputs.
- Avoid reshaping or relaying out cells whose percentage dependency set is
  empty.

### Evidence

- Pure fixtures cover definite and indefinite table heights; percentage rows;
  percentage cells; percentage children and grandchildren; replaced content;
  overflow modes; nested tables; and direct cycles.
- Pass counters prove independent cells remain single-pass.
- Final `FragmentTree` contains one unfragmented subtree per cell rather than
  both measurement and final-pass fragments.
- Focused WPT includes:
  - `height-distribution/percentage-sizing-of-table-cell-*`;
  - `percent-height-table-cell-child`;
  - `percent-height-overflow-auto-*`;
  - percentage replaced-cell cases; and
  - table-as-item cell-percentage cases affected by the block basis.
- Tentative percentage cases remain labeled as such in the receipt.

### Stop rules

- Stop if an indefinite percentage becomes a viewport percentage.
- Stop if the second pass reuses stale child fragments or overflow.
- Stop if the algorithm has an unbounded stabilization loop.
- Stop if every cell is relaid out to make percentage cases pass.
- Stop if a tentative WPT is reported as stable standards acceptance.

### Removal receipt

Delete any generic backend percentage-height retry used only to compensate
for table-as-Grid layout. Retain shared formatting-context relayout support
only when another non-table consumer is proved.

### K4d4 interop matrix - 2026-08-01

Measured with `interop-matrix.html` in the K4d4 proof directory, headless
**Chrome 150.0.0.0** and **Firefox 153.0.1**. `inner` is the measured
subject (row heights, or the percentage descendant's height); `outer` is its
table or cell.

| Case | Chrome 150 | Firefox 153 |
|---|---|---|
| P1 pct row 50%, table auto | rows 20 / 40, table 60 | same |
| P2 pct row 50%, table 300 | rows 150 / 150 | same |
| P3 pct cell 50%, table auto | rows 20 / 40, table 60 | same |
| P4 pct cell 50%, table 300 | rows 150 / 150 | same |
| P5 pct child, cell auto, table auto | child 0, cell 40 | same |
| P6 pct child, cell 100px | child 50, cell 100 | same |
| **P7 pct child, cell auto, table 300** | **child 150**, cell 300 | **child 0**, cell 300 |
| P8 pct grandchild through auto wrapper, cell 100px | child 0, cell 100 | same |
| P9 pct cell holding a pct child, table auto | child 0, cell 0 | same |
| P10 pct child in `overflow:auto` cell 100px | child 50, cell 100 | same |

**Nine of ten cases agree.** The accepted rules:

1. A percentage row or cell height resolves only against the table's
   *specified* definite block size, never against the used size K4d3 just
   computed from content (P1, P3 versus P2, P4). That distinction is what
   makes the pass acyclic: a percentage never feeds the height it measures
   against.
2. A percentage row height and a percentage cell height behave identically
   (P2 equals P4).
3. A percentage descendant with no basis is zero, not automatic (P5).
4. A descendant's basis is the cell's final used block size, and only its
   direct children see it: an intervening auto-height wrapper breaks the
   chain in both engines (P8). Buckram supplies the cell block size and the
   content formatting context applies the ordinary chain rule, so P8 needs no
   table-specific code.
5. `overflow: auto` on the cell does not change the basis (P10).

**The one divergence, P7,** is resolved toward Chrome: a cell whose own
height is automatic but whose used height is made definite by the table does
give its percentage children a basis. Firefox requires the cell's own
specified height to be definite. Chrome's behavior is what this gate is
architecturally for, since the plan's second pass exists precisely to apply a
basis that appears only after row distribution; under Firefox's rule that
pass would be unreachable for automatic cells. CSS 2.1 section 17.5.3 also
makes a cell's height a product of its row, so the used height is a genuine
basis once rows are final.

**`PercentageBlockCycle` is unreachable from this path, and that is a
result rather than an omission.** Because a percentage never resolves against
a used height it helped produce (rule 1), the two passes cannot feed each
other. P9's apparent cycle collapses to zero in both engines for the same
reason: no basis ever appears. The variant stays reserved for a future gate
that introduces a genuine dependency loop.

### K4d4 receipt - 2026-08-01

**Base commit:** `dddd01c537b` (accepted K4d3).

**Capability:** `resolve_percentage_block_sizes` performs the bounded second
pass. Percentage row and cell constraints resolve against the specified table
basis and row sizing re-runs once; then each cell whose contents depend on
its block size is reformatted exactly once with
`TableCellLayoutPass::ResolvePercentages`, replacing its first-pass drafts
and overflow outright. `TableCellBlockStyle` gained
`percentage_dependent_contents` so a cell with an empty dependency set is
never touched.

**Boundedness:** the second format pass never re-drives row sizing. Growth
discovered there would start an unbounded stabilization loop, and neither
engine grows a row for it. Two named passes, no iteration, no convergence
test.

**Interop decision:** the matrix above, including P7 at Chrome's value.

**Pure fixtures:** P1 through P4 as measured; the P6 and P7 descendant-basis
pair; P9's unbased chain collapsing without iteration; and the plan's pass
counter, proving a cell with no percentage dependency is never reformatted
while a dependent cell is reformatted exactly once. Four tests; buckram 143.

**WPT:** both table corpora rerun against the K4d3 maps with zero movement,
as a model-only gate requires. `percent-height-table-cell-child.html` is
captured as pre-cutover orientation and currently fails; K4d6 is where it can
move. Proof directory:
`testing/genet/wpt-ledger/2026-08-01_buckram_k4d4`.

**Verification:** buckram 143, livery and genet-livery all targets 0 failed;
clippy clean on touched files; exact-file Rustfmt and `git diff --check`
clean.

## K4d5. Cell alignment and table baseline sets

### Outcome

Place cell content within the final row geometry and export standards-owned
table baselines.

### Work

- Derive each cell baseline from the formatting-context output:
  - first in-flow line box;
  - otherwise first in-flow table row;
  - otherwise the cell's bottom content edge.
- Treat scrolling content as being at its initial scroll position for baseline
  selection.
- Establish the row baseline from the largest block-start-to-baseline distance
  among baseline-aligned cells.
- When a row has no baseline-aligned cell, synthesize from the lowest cell
  content edge under the accepted CSS2 rule.
- Apply CSS2's ordered alignment:
  1. baseline cells;
  2. top cells;
  3. any row growth required by bottom and middle cells; and
  4. final bottom and middle placement.
- Map `sub`, `super`, `text-top`, `text-bottom`, lengths, and percentages to
  baseline behavior for table cells as CSS2 requires.
- Model extra block-start and block-end fill separately from computed cell
  padding.
- Export the table's first baseline from its first row and its last baseline
  from its last row under the accepted CSS Box Alignment model.
- Keep baselines as logical offsets through vertical and orthogonal writing
  modes.

### Evidence

- Pure fixtures cover every table-cell `vertical-align` value; cells with and
  without line baselines; nested tables; scroll containers; spanning cells;
  content taller than the provisional row; and empty rows.
- Baseline changes do not alter K4c column sizes.
- Adapter fixtures prove flex, grid, block, and inline cell contents return
  baselines directly.
- Focused WPT includes:
  - `baseline-vertical`;
  - `baseline-empty-cell-*`;
  - CSS2 `table-vertical-align-baseline-*`;
  - `table-cell-baseline-static-position`, classified for its K4h positioning
    dependency; and
  - current first and last table-baseline cases.

### Stop rules

- Stop if a baseline is recovered by walking backend descendants.
- Stop if alignment mutates computed padding values.
- Stop if a physical `y` coordinate is stored as a baseline.
- Stop if a rowspan participates in a row baseline outside the first or last
  row required by the selected baseline rule.

### Removal receipt

Delete generic synthesized table and row baselines. `TableRowLayoutResult`
becomes the only baseline source for table-internal and table-grid fragments.

### K4d5 receipt - 2026-08-01

**Base commit:** `8c87ec62dc1` (accepted K4d4).

**Capability:** `align_table_cells` applies CSS 2.1 section 17.5.3's ordered
procedure and exports the table's baseline set.
`apply_baseline_row_minima` is its necessary companion.

**The gate's real finding: baseline alignment is a row-growth step, and
K4d2's minima cannot see it.** A row must hold the deepest cell above the
shared baseline plus the deepest below it, and that sum can exceed the
tallest single cell. Two cells of 50px (baseline 10) and 40px (baseline 30)
produce a 70px row. So `apply_baseline_row_minima` runs between K4d2's
content minima and K4d3's sizing, and the ordering the plan asks for
(baseline cells, then top, then growth, then bottom and middle) is expressed
as that pipeline position rather than as a re-entrant pass.

**Baseline model selected:** CSS Box Alignment 3's baseline sets. The table's
first baseline comes from its first row and its last from its last row. CSS
2.1 section 10.8 defines only the first, as an inline table's baseline, and
that remains the first entry of the same set. Offsets stay logical
throughout; nothing stores a physical coordinate as a baseline.

**Other accepted rules:**

- A spanning cell's baseline belongs to the row it starts in and never
  participates in a later row's, per the gate's stop rule.
- A row with no baseline-aligned cell, or whose baseline-aligned cells report
  no line baseline, synthesizes from the lowest cell content edge, and
  `TableRowBaseline::from_aligned_cell` records which happened.
- Alignment is a placement offset. It never mutates the computed padding that
  produced the cell's offsets, and the extra block-start fill is a separate
  `content_block_offset`.

**Adapter lowering:** `table_cell_alignment` collapses `vertical-align` to
the four behaviors CSS 2.1 gives a table cell. `sub`, `super`, `text-top`,
`text-bottom`, lengths, and percentages become `baseline` at the adapter, so
Buckram never receives a distinction its algorithm would have to ignore.

**Pure fixtures:** baseline growth beyond the tallest cell; all four
alignments placed in one row; synthesis both when no cell is baseline-aligned
and when a baseline-aligned cell has no line box; first and last table
baselines under non-zero spacing; a spanning cell confined to its starting
row; and column sizes proved unchanged. Six tests; buckram 149.

**Adapter fixture:** `k4d5_cell_contents_return_baselines_directly` formats
block cell contents over the live `AlgorithmTree` and aligns from the
baselines the formatting context returns, with no descendant walk. It also
pins the correction below.

**Correction found while writing that fixture:** a block container's *first*
baseline is its first child's, so two cells differing only in child count
share a baseline. The fixture now varies the first child's height, which is
what actually moves a first baseline.

**WPT:** both table corpora rerun against the K4d4 maps with zero movement,
as a model-only gate requires. Proof directory:
`testing/genet/wpt-ledger/2026-08-01_buckram_k4d5`.

**Verification:** buckram 149, livery and genet-livery all targets 0 failed;
`cargo clippy -p buckram -- -D warnings` clean. The combined command remains
blocked by the pre-existing unrelated `components/genet-livery/src/text.rs`
warning, which this gate does not touch. Exact-file Rustfmt and
`git diff --check` clean.

### K4d4b interop matrix - 2026-08-03

K4d6b's live cutover exposed a case K4d4's matrix never covered: a
**percentage cell height whose row has a definite specified height**. Every
K4d4 case gave the percentage cell an automatic row, so nothing distinguished
"resolve against the table" from "resolve against the table, then fit the
table". Measured with `interop-matrix.html` in the K4d4b proof directory,
headless **Chrome 150.0.0.0** (`--dump-dom`) and **Firefox 153.0** (`--screenshot`).

| Case | Chrome 150 | Firefox 153 |
|---|---|---|
| Q1 table 100, rows 50/50, cells 100% | rows 50 / 50, table 100 | same |
| Q2 table 300, row 50, cell 100% | row 300 | same |
| Q3 table auto, row 80, cell 50% | row 80 | same |
| Q4 control: table 300, auto row, cell 50% | rows 150 / 150 | same |
| Q5 table 400, row 20, cell 50% | rows 200 / 200 | same |
| Q6 table 200, row 25%, cell 100% | rows 190 / 10 | same |
| Q7 table 200, spacing 10, row 60, cell 100% | row 180 | same |

**All seven agree**, so there is no divergence to resolve, and one rule
accounts for every case:

> A percentage row or cell height still resolves only against the table's
> specified definite block size, exactly as K4d4 accepted. What K4d4 missed
> is that the resulting growth is then **fitted back into that height**.

K4d3's rule that a definite table block size is a *minimum* stands, and this
does not contradict it. That rule is about rows sized by content or by their
own length, which cannot give the space back. Percentage-derived growth was
computed *from* the table's height, so letting it overflow doubles the table:
Q1 is exactly `table-as-item-cell-percentage-002`, where two 50px rows with
`height: 100%` cells produced a 200px table instead of a 100px square.

`shrink_percentage_growth` implements it. Each row shrinks only across the
distance between its K4d2 minimum and what the resolved percentages asked
for, in proportion to that distance, and never below the minimum. A row that
grew for content or a length has zero growth here and is untouched. Q6 is
the case that pins the proportion to the *pre-distribution* minima rather
than the first pass's sizes: floors of 50 and 10 against a demand of 200 and
10 give 190 and 10, while measuring growth from an already-distributed first
pass would give 95 and 105.

### K4d4c interop matrix - 2026-08-03

`min-height` and `max-height` on table cells and rows. CSS 2.1 section 10.7
says their effect on tables, inline tables, table cells, table rows, and row
groups is **undefined**, so K4d6b's first pass deferred them under a named
`UnmodeledConstraint` skip rather than dropping a declaration silently.
Measured with `interop-matrix.html` in the K4d4c proof directory, headless
**Chrome 150.0.0.0** and **Firefox 153.0**.

| Case | Chrome 150 | Firefox 153 |
|---|---|---|
| R1 cell `max-height: 20`, content 100 | cell 100 | same |
| R2 cell `max-height: 300`, content 100 | cell 100 | same |
| R3 cell `min-height: 150`, content 40 | cell 40 | same |
| R4 cell `height: 20` + `max-height: 20`, content 100 | cell 100 | same |
| R5 row `max-height: 20`, content 100 | cell 100 | same |
| R6 row `min-height: 150`, content 40 | cell 40 | same |
| R7 table 300, cell `max-height: 20`, content 40 | cell 300 | same |
| R8 cell `max-height: 20` + `overflow: hidden` | cell 100 | same |

**All eight agree: both properties are ignored outright**, on cells and on
rows, whether they would grow or shrink the box, and `overflow` does not
change it. So this is modeled behavior, not a gap, and the fix is a
deletion: the two `UnmodeledConstraint` skips and the cell's `min-height`
border-box floor are gone, and no table defers for either property.

## K4d6. Fragment emission, live dispatch, and bridge deletion

**Split into K4d6a and K4d6b.** K4d6a emits the table's fragment subtree as a
model, with no live behavior change and a zero-movement receipt. K4d6b routes
live tables through the dispatcher, commits those fragments, and deletes the
bridge. This is the same fault line K4c5 was split along, for the same
reason: landing the cutover together with the removal of the code that would
reveal a regression makes any movement unattributable. K4d6a's outcome,
evidence, and stop rules are the fragment-model subset of the list below;
K4d6b owns the live dispatch, the deletions, and the removal receipt.

### K4d6a receipt - 2026-08-01

**Base commit:** `d0fd1f3840b` (accepted K4d5).

**Capability:** `components/buckram/src/table/fragments.rs` emits one
fragment per table-internal box from the accepted K4c inline result, K4d3 row
sizing, and K4d5 alignment: grid, row groups, rows, column groups, columns,
and cells. Each carries its logical border-box rectangle, its structural
parent, and its overflow. Parents always precede their children in the
vector, so K4d6b can commit in order without a second traversal.

**Structural parents** follow the plan: groups under the grid, rows under
their row group or directly under the grid when ungrouped, columns under
their column group or the grid, and cells under the row they originate in.

**Rectangles come from the track model, never from painted cells.** A row
group's rectangle is the exact union of its track range, and a column exists
with its own rectangle even where no cell occupies it, which the
`a_column_without_cells_still_has_its_own_rectangle` fixture pins. That is
the gate's stop rule about reconstructing a column or group during paint,
enforced by construction rather than by review.

**A spanning cell gets exactly one fragment** covering its whole row range.
Nothing is split here; K6 owns the fragmented case.

**Overflow** unions upward from each cell through its row, that row's group,
and the grid, in a single reverse sweep, and never disturbs any fragment's
own rectangle.

**Emitted, not committed.** `TableFragments` is a Buckram value with no path
into a `FragmentTree`, so K4d6a cannot change painted output by construction,
exactly as K4d1's draft discipline requires.

**Fixtures:** every role present with correct parents and counts; group and
column-group rectangles as exact track unions; a column with no cells; the
overflow union; a spanning cell's single rectangle; and collapsed metrics
deferring before any fragment is emitted. Five tests; buckram 154.

**WPT:** both table corpora rerun against the K4d5 maps with zero movement.
Proof directory: `testing/genet/wpt-ledger/2026-08-01_buckram_k4d6a`.

**Verification:** buckram 154, livery and genet-livery all targets 0 failed;
`cargo clippy -p buckram -- -D warnings` clean; exact-file Rustfmt and
`git diff --check` clean.

### K4d6b entry notes - 2026-08-01

Scoped while closing K4d6a, so the cutover starts from a sized problem
rather than a discovery phase.

**The adapter has no way to write Buckram geometry back.** `AlgorithmTree`
exposes `style_mut`, `baselines`, and `set_baselines`, but no `set_layout`.
K4d6b needs one: the pipeline computes every table-internal rectangle before
`compute_layout` runs, and the table's arm must then return that size without
recursing, or Taffy will overwrite the children it should not own.

**Rows, row groups, columns, and column groups have no algorithm nodes
today.** The bridge flattens them away, so cells are direct children of the
table node. K4d6a's fragments cover all six roles, so the cutover must either
create structural nodes for them or splice the emitted subtree into
`collect_fragments` directly. The second is closer to the plan's wording
("record each content-containing fragment separately from its structural
parent") and avoids inventing backend nodes for boxes that never enter a
backend algorithm.

**`AlgorithmKind::Table` currently falls to `compute_hidden_layout`,** which
is a zero rectangle. It is now behind a `debug_assert!` so a premature
constructor fails loudly in tests instead of silently collapsing a table.

**This is the first K4d gate without a zero-movement safety net.** Its
receipt has to classify every moved test and run the complete all-nine
comparison if anything outside the two table corpora moves, which is the same
shape of work as the 2026-07-31 split-fix attribution. Budget it as its own
effort rather than as a tail on another gate.

#### The owned-context seam has landed

`AlgorithmTree::set_layout` writes a rectangle a Buckram algorithm already
decided, and the `AlgorithmKind::Table` arm reports that size without laying
out children, so the backend cannot overwrite what the table algorithm owns.
`an_owned_table_context_keeps_the_geometry_it_was_given` proves both halves
against a real backend walk.

**The rectangle must be written unrounded.** The adapter keeps two stores:
algorithms write `unrounded_layout`, and the backend's rounding pass then
walks the entire tree from the root and derives every `final_layout` from it.
Writing only `final_layout` is silently discarded by that pass, which is
exactly what the first version of this seam did. Writing a pre-rounded value
instead would round the table's subtree on a different grid from its
siblings, so `set_layout` writes unrounded and lets the shared pass round it.

#### Buckram owns the order of the phases

K4d1 through K4d6a each accepted one phase as its own public function so it
could be gated on its own evidence. That left the *order* unowned, and the
order is load-bearing at two points: baseline minima are a row minimum
content measurement cannot see, so they must reach row sizing, and the
percentage pass may grow rows again, so alignment and fragment emission must
both read its sizing rather than the first pass's. An adapter that sequenced
those wrong would produce a different table with no error anywhere.

`components/buckram/src/table/pipeline.rs` owns that order once.
`layout_table_block` runs format, measure, baseline minima, size, percentage
pass, align, and emit, and returns `TableBlockLayout`: the final sizing,
alignment, per-cell outputs, the cells the percentage pass relaid out, and
the emitted fragment subtree. Both order dependencies are pinned by fixtures
that fail under the wrong sequence rather than by review.

`TableRowLayoutResult` is **removed**. It was declared in K4d1 as the
eventual result shape and nothing ever produced it; `TableBlockLayout` is
what the pipeline actually returns, and its fields are accepted types from
the phases that produce them. An exported result type no code path can
produce reads as a capability receipt for a driver that did not exist.

`buckram_table_columns` now returns the whole `TableInlineSizingResult`
rather than just its column vector, because `TableBlockSizingInput` takes
that result as its inline input. Re-deriving the used grid width or the
undistributable remainder from the columns alone would put K4c's arithmetic
back in Livery.

#### The live block pipeline runs in shadow

Following K4c5's split, the pipeline runs on every live table before it is
given authority. `components/genet-livery/src/table_block.rs` lowers the
block axis and calls `layout_table_block`; `verify_table_block` then compares
Buckram's cell rectangles against the painted fragments after collection.
Nothing is written to the tree yet, so this step cannot move a WPT test, and
it converts the cutover's open-ended risk into a measured distance.

**The measurement immediately found a live capability gap, in Buckram's
favor.** A `height` on a `<tr>` is a row minimum under CSS 2.1 section
17.5.3. The bridge flattens rows away before the backend sees them, so that
declaration reaches no grid track: for a two-row table with rows of 40px and
60px, the bridge paints 18px and 19px content-height rows while Buckram
produces 40 and 60. Every divergence recorded in
`k4d6b_buckram_rows_honor_row_heights_the_bridge_drops` is the bridge falling
short, which is asserted directly rather than described. The cutover is
therefore a capability gain, and its WPT movement should be improvements.

**Deferrals are named, never silent.** Collapsed borders defer to K4g as
before. Two new named skips cover what the block axis has no contract for: a
cell or row `max-height` (`UnmodeledConstraint`), which is dropped by no one
because dropping it would silently change the table, and a percentage
block-axis padding. That padding's basis is the table box's content width,
which Livery cannot name from K4c's result alone, because the
undistributable remainder folds the table's own padding and border together
with separated spacing. Picking one of the two plausible bases would be
exactly the invented geometry this boundary exists to prevent, so it defers
under the gap the inline axis already uses and the ledger counts it.

**The block ledger nests inside `TableShadowLedger`** rather than running
parallel to it. A table's two axes are one dispatch decision, and two ledgers
threaded through three build routes would drift apart.

Both build routes now run it: the inline route that production uses, and the
block-only route. K4c5b's finding that `fixed_column_widths` had never run in
production came from exactly that asymmetry.

**WPT:** both table corpora rerun against the K4d6a maps with zero movement.
`css/css-tables` 56 passed, 74 failed, 198 skipped; `css/CSS2/tables` 68
passed, 182 failed, 889 skipped; `unexpected=0` on both. Proof directory:
`testing/genet/wpt-ledger/2026-08-02_buckram_k4d6b_shadow`. Those maps are
also the baseline the cutover will be classified against.

#### The cutover is written and measured, on branch `k4d6b-cutover`

Commit `fe6a7114a9c` writes every cell rectangle through the owned-context
seam, sizes the table node from `used_table_block_size`, and switches its
dispatch to `AlgorithmKind::Table`. A table Buckram defers keeps the Grid
bridge it was built with, which is what `AlgorithmTree::set_kind` is for:
whether Buckram can lay a table out is only known once the algorithm has
run, so deciding at build time would mean guessing or giving up the
fallback.

It is held off `main` because it moves nine reftests and one of them is a
**model defect it exposed rather than caused**. That defect is the blocker,
and it belongs to K4d4:

> `table-as-item-cell-percentage-002` expects a 100px square and renders
> 100x200. With `table { height: 100px }`, `tr { height: 50px }`, and
> `td { height: 100% }`, the cell percentage resolves against the table's
> specified height rather than its row's, so each 50px row grows to 100 and
> the table doubles. The test's own assertion is that cells "do not
> re-resolve their percentage heights based on the table's height".

K4d4 chose the table's specified block size as the basis for both rows and
cells, on measured evidence. That is right for a **row** percentage and
wrong for a **cell** percentage, whose basis is its row's height when the
row has a definite one. Correcting it needs the same browser measurement
K4d4 was built on, not a guess, so it is the next gate's work rather than a
patch here. `001`, `003`, and `004` in the same family all **improve**,
which is what makes the single regression readable as a basis error rather
than a broken pass.

The rest of the movement is classified:

- **Improvements (3):** `table-as-item-cell-percentage-001`, `-003`, `-004`.
- **False-pass disclosures (3):** `separated-border-model-007`, `-008`,
  `-009`. Before the cutover these rendered **zero red pixels** because the
  table painted nothing at all; the reference is "no red", so an invisible
  table passed. Now the table paints correct geometry (rows at 48 and 128,
  table 208 tall, all verified) and the reftest exposes a **separate
  pre-existing bug**: an absolutely positioned box at `top`/`left: 16px`
  under `body { margin: 16px }` lands at 32,32 instead of 16,16, so its
  containing block is the body's content box rather than the initial
  containing block. Proved with a table-free probe, so it owes nothing to
  K4. It needs its own gate outside K4.
- **1px rounding (1):** `colspan-004` shifts one row by 1px. Its column
  distribution, which is what the test targets, is byte-identical before and
  after at 7/96/7 rather than the correct 5/100/5, so that colspan defect is
  pre-existing K4c work this does not touch.
- **Unexamined (1):** `table-cell-overflow-explicit-height-002`.

**Baselines:** `testing/genet/wpt-ledger/2026-08-02_buckram_k4d6b` holds the
post-cutover maps, against the `_shadow` maps beside them.

What remains: fix K4d4's cell-percentage basis with browser measurement,
re-measure the two corpora, examine the last unclassified test, splice
K4d6a's structural fragments into `collect_fragments`, and delete
`place_table_cell`, `table_is_flattenable`, and the table-to-Grid and
row-to-Flex mappings in `algorithm_kind` and `anonymous_taffy_style`.

### Outcome

Commit the final table fragment subtree, switch live tables to Buckram table
dispatch, and remove the Grid/Flex bridge.

### Work

- Emit logical fragments for the table grid, row groups, rows, column groups,
  columns, cells, and anonymous missing cells represented by K4b.
- Give spanning cells one unfragmented fragment covering their complete row
  and column range.
- Give row and column groups the exact union of their track ranges rather than
  a rectangle reconstructed later from painted cells.
- Preserve structural parent relationships:
  - grid under the K4a wrapper;
  - groups and ungrouped rows or columns under the grid;
  - rows under their row group or grid;
  - columns under their column group or grid; and
  - cells under their originating row.
- Record each content-containing fragment separately from its structural
  parent. K4h later finalizes positioned containing-block behavior.
- Union child and border-model overflow into cell, row, group, and table
  overflow.
- Commit only the final cell-layout pass's fragment drafts.
- Route every live table through the Buckram table dispatcher.
- Delete `place_table_cell`, `table_is_flattenable`, table-to-Grid,
  row-to-Flex, and all table-specific Taffy style mutations.
- Keep relative row, group, and cell fragments on the table path with their
  unshifted normal-flow geometry. K4h applies the relative offset.

### Evidence

- Pure fixtures assert every fragment role, logical rectangle, structural
  parent, containing fragment, first and last baseline, and overflow union.
- Adapter counters prove table and row backend dispatch is zero while flex and
  grid contents inside cells still reach Taffy.
- Live fixtures cover anonymous rows and cells, all row-group roles, columns,
  groups, rowspans, colspans, fixed and automatic widths, nested tables,
  vertical writing, and relatively positioned table parts.
- A source audit finds no table-to-Grid or row-to-Flex selection in
  `genet-livery` or `buckram::taffy_adapter`.
- Fresh complete expectation maps for `css/CSS2/tables` and
  `css/css-tables` are compared exactly with every accepted K4d gate.
- Any movement in another K3 ratchet directory triggers the complete all-nine
  comparison.

### Stop rules

- Stop if a positioned row returns to the old nesting or Grid/Flex bridge.
- Stop if one DOM node lookup is used as a substitute for fragment identity.
- Stop if a column or group rectangle is reconstructed during paint.
- Stop if K4d claims fragmentation, repeated headers, absolute positioning,
  captions, or collapsed-border completion.

### Removal receipt

The accepted K4d6 tree contains:

- one Buckram table dispatcher;
- zero table Grid dispatches;
- zero row Flex dispatches;
- no `place_table_cell`;
- no `table_is_flattenable`;
- no table-specific Taffy placement mutation; and
- complete unfragmented table-internal fragment identity.

K4h audits that this bridge remains absent. It does not postpone the deletion.
K4f still supplies visibility-collapse masks through the accepted K4c and K4d
inputs.

## Cross-gate dependency map

| Consumer | K4d output or input |
|---|---|
| K4c sizing | supplies final column sizes; receives no completed row rectangles |
| K4e wrapper and captions | inserts the K4d grid fragment subtree into wrapper flow |
| K4f separated rendering | supplies track-visibility masks, reruns K4c/K4d, and consumes the resulting fragments |
| K4g collapsed borders | supplies block-axis winning-border metrics, then reruns K4d row layout |
| K4h positioned tables | applies relative offsets and exposes final positioned containing fragments |
| K5 positioning | consumes preserved table fragment identity for absolute, fixed, and sticky work |
| K6 fragmentation | splits K4d's unfragmented rows and cells and owns repeated headers and split rowspans |

K4g is a border-metric completion, not a second row algorithm. Once its
winning borders exist, K4d reruns with collapsed block metrics.

## Global acceptance invariants

K4d is complete when all of these are true:

1. Every live table uses a Buckram table dispatcher.
2. Every row has one finite non-negative logical size.
3. Row sums and active border-model space reconcile with the table block size.
4. Every spanning cell fits its accepted row range.
5. Percentage passes are bounded and basis-aware.
6. Cell baselines come from formatting-context outputs.
7. Cell alignment does not mutate computed padding.
8. Every table-internal role has a stable fragment and structural parent.
9. Positioned table parts remain on the table path.
10. Table Grid and row Flex dispatch counts are zero.

## Verification ladder for every sub-gate

1. **Model proof:** pure Buckram fixtures name the CSS distinction.
2. **Adapter proof:** cell formatting crosses a callback boundary using
   Buckram types.
3. **Live proof:** accepted row sizes, baselines, fragments, and counters
   reach Livery where the gate claims integration.
4. **Focused corpus:** fresh exact maps cover the named WPT families.
5. **Regression ratchet:** exact comparison against the preceding accepted
   gate, including complete CSS2 whenever CSS2 moves.
6. **Interop receipt:** every CSS2-undefined distribution records current
   browser and WPT evidence.
7. **Build proof:**

   ```powershell
   $env:CARGO_TARGET_DIR = 'C:\t\graphshell-target'
   cargo test -p buckram -p livery -p genet-livery --offline
   cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings
   rustfmt --edition 2024 --check <touched Rust files>
   git diff --check
   cargo build -p genet-wpt --release --all-features --offline
   ```

Run Rustfmt only on touched Rust files. If the combined Clippy command is
blocked by a pre-existing unrelated warning, run each touched crate
separately and record the exact blocker.

Store generated expectation maps and browser measurements under:

`<workspace>\testing\genet\wpt-ledger\<date>_buckram_k4d<gate>`

Keep proof outputs out of Git.

## Receipt template

Append one receipt beneath the completed sub-gate:

```markdown
### K4dN receipt - YYYY-MM-DD

Base commit:

Capability:

Boundary retained:

Pure fixture:

Adapter fixture:

Live fixture:

Interop decision:

WPT exact movement:

Dispatch and deferral counts:

Removal:

Verification:

Proof directory:

Commit:
```

## First executable task

The initial handoff is:

> Read this plan, the accepted K4c receipt, CSS 2.1 section 17.5.3, the CSS2
> inline-table baseline rule, and the live seams named under
> K4d1. Execute K4d1 only. Preserve unrelated worktree changes. Record the
> accepted K4c commit and freeze fresh `css/CSS2/tables` and
> `css/css-tables` maps before changing layout behavior. Stop after K4d1
> passes its verification ladder, append its receipt here, stage only K4d1
> paths, and commit. Do not begin K4d2 in the same task.
