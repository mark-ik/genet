# Buckram K4d table row layout execution plan

**Date:** 2026-07-28

**Status:** scoped; execution starts after the accepted K4c receipt

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

## K4d6. Fragment emission, live dispatch, and bridge deletion

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

`C:\Users\mark_\Code\testing\genet\wpt-ledger\<date>_buckram_k4d<gate>`

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
