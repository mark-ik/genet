# Buckram K4c table inline sizing execution plan

**Date:** 2026-07-28

**Status:** ready. The accepted K4b base is `26eda4cd9fe`; K4c1 is next.

**Parent plan:** [Buckram K4 CSS tables execution plan](2026-07-28_buckram_k4_css_tables_execution_plan.md)

**Architectural authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

## Ruling

K4c makes Buckram the sole owner of table, grid, and column inline sizes.

The sizing algorithm consumes the standards-shaped `TableGrid` produced by
K4b, computed table and cell constraints, and Buckram intrinsic-size queries.
It returns intrinsic table sizes, a used table-grid inline size, and one used
size per column. Taffy may format content inside a cell. It does not infer
table tracks, measure a completed grid, or choose a table width.

K4c closes these concerns:

- fixed table layout in the separated-border model;
- automatic cell and column measures;
- automatic used table width and column distribution;
- intrinsic table widths for parent formatting contexts; and
- live consumption of Buckram's column sizes by the temporary placement
  bridge.

K4c does not close row heights, cell block alignment, captions, collapsed
border winners, table painting, positioning, or fragmentation. Its contracts
must let those gates add their inputs without replacing the sizing algorithm.

## Entry gate

Execution starts from an accepted K4b commit and receipt, not directly from
the K3 closure commit.

K4b must already provide:

- a `TableGrid` with stable table-grid, row, column, group, and cell identity;
- normalized `(row_start, column_start, row_span, column_span)` placement;
- explicit column and column-group membership;
- normalized HTML `colspan`, `rowspan`, and `span` inputs; and
- an exact count of tables still routed through the Grid/Flex compatibility
  bridge.

The frozen pre-K4 orientation is the K3 closure commit
`2f1ae56968c` (`Complete Buckram K3 closure ratchet`). K4c starts from the
accepted K4b commit `26eda4cd9fe` and produces fresh expectation maps for:

- `css/CSS2/tables`; and
- `css/css-tables`.

The K3 table counts are orientation only:

| Corpus | Pass | Fail | Skip | Error |
|---|---:|---:|---:|---:|
| `css/CSS2/tables` | 66 | 184 | 889 | 0 |
| `css/css-tables` | 50 | 80 | 198 | 0 |

## Standards boundary

[CSS 2.1 section 17.5.2.1](https://www.w3.org/TR/CSS2/tables.html#fixed-table-layout)
is the stable authority for fixed layout. Its ordering is an algorithm:

1. a non-automatic column width establishes that column;
2. otherwise, a non-automatic first-row cell width establishes its column or
   is divided across its span;
3. remaining columns share the remaining horizontal table space after
   borders and spacing; and
4. later rows do not change column widths.

The used table width is at least the width required by its columns, borders,
and spacing. Extra space is distributed over the columns.

[CSS 2.1 section 17.5.2.2](https://www.w3.org/TR/CSS2/tables.html#auto-table-layout)
defines constraints for automatic layout but deliberately makes the detailed
algorithm non-normative. K4c treats the following as stable invariants:

- cells contribute minimum and maximum content widths;
- single-column cells constrain their column before spanning cells;
- spanning cells and column groups constrain the columns they cover;
- percentage widths remain constraints until their basis is definite;
- the table's minimum width includes column minima and undistributable
  border-model space; and
- used width depends on the table's specified width, containing block,
  intrinsic bounds, and minimum caption contribution.

The current [CSS Tables Level 3 draft](https://drafts.csswg.org/css-tables-3/)
is an interoperability design input. It is not sufficient authority for an
underspecified choice. In particular, its current spanning-cell distribution
text contains an editorial warning that the described algorithm is wrong.
K4c must not transcribe that text into Buckram without current browser and WPT
evidence.

For each non-normative choice, add an interop decision record to the gate
receipt containing:

- the exact WPT or reduced fixture;
- browser names and build versions;
- containing, table, cell, and column widths observed;
- the competing rules considered; and
- the rule selected for Buckram.

## Live debt at the entry seam

The current Livery path contains a useful proof and several deletion targets:

- `fixed_column_widths` recognizes `table-layout: fixed` only when the table
  has a definite width.
- It derives the column count by walking flattened DOM cells.
- It reads only first-row cell widths.
- It omits columns, column groups, spans, border spacing, automatic layout,
  and table intrinsic sizes.
- It writes its result into Taffy `grid_template_columns`.
- `horizontal_edges` converts cell padding and borders to a physical
  horizontal number on the adapter side.
- Taffy still auto-sizes all cases for which the helper returns `None`.

The first-row arithmetic remains a regression fixture. The DOM walk, physical
edge helper, and Taffy-owned fallback are not part of the target design.

## Sizing contracts

The exact Rust spelling may change during K4c1. The ownership and information
boundaries may not.

```rust
pub struct TableInlineSizingInput<'a> {
    pub grid: &'a TableGrid,
    pub available_inline_size: Option<f32>,
    pub table_constraints: TableInlineConstraints,
    pub border_metrics: TableInlineBorderMetrics,
    pub caption_min: CaptionMinContribution,
    pub track_visibility: TableTrackVisibility,
}

pub struct TableCellInlineMeasure {
    pub box_id: BoxId,
    pub content: IntrinsicSizes,
    pub preferred: InlineSizeConstraint,
    pub minimum: InlineSizeConstraint,
    pub maximum: InlineSizeConstraint,
    pub offsets: CellInlineOffsets,
}

pub struct TableColumnMeasure {
    pub min_content: f32,
    pub max_content: f32,
    pub intrinsic_percentage: f32,
    pub constrained: bool,
}

pub struct TableInlineSizingResult {
    pub intrinsic_sizes: IntrinsicSizes,
    pub used_table_inline_size: f32,
    pub used_grid_inline_size: f32,
    pub column_sizes: Vec<f32>,
}
```

The types must preserve these distinctions:

1. `auto`, definite lengths, percentages, affine length-percentage values,
   and intrinsic keywords remain distinct until their bases are available.
2. Content contribution, padding, and border contribution are separate.
   K4g can therefore substitute resolved collapsed-border half-widths without
   replacing K4c's column algorithms.
3. Separated-mode undistributable space is a table-level input derived from
   the number of columns and inline border spacing.
4. `CaptionMinContribution` distinguishes no caption, a measured
   contribution, and a named K4e deferral. An unknown caption contribution
   must not silently become zero.
5. `TableTrackVisibility` defaults every track to visible. K4f can collapse
   tracks without deleting the constraints that produced their pre-collapse
   measures.
6. A result has exactly one non-negative finite size per K4b column.
7. `IntrinsicSizes::min_content <= IntrinsicSizes::max_content`.
8. The used grid size equals the sum of used column sizes plus the
   undistributable border-model space, within an explicitly tested subpixel
   tolerance.

K4c may add table-specific reasons around `IntrinsicQueryError`, but it must
preserve the existing explicit cycle, indefinite-basis,
fragmentation-dependent, and unsupported-axis outcomes. Expected additional
outcomes include:

- unresolved percentage basis;
- unresolved collapsed-border geometry;
- pending caption minimum; and
- a CSS size expression that cannot yet be reduced without losing its
  semantics.

Failures are not cached as intrinsic answers.

## Border-model seam

Sizing and border rendering land in different K4 gates, but they share
geometry.

In separated mode, K4c can compute:

- each cell's logical inline padding and border offsets; and
- the undistributable inline border spacing for the K4b column count.

K4c therefore accepts the separated `fixed-table-layout-003a*`,
`fixed-table-layout-003b*`, and `fixed-table-layout-003c*` families.

In collapsed mode, cell intrinsic offsets depend on the winning border on
each grid edge. K4g owns those winners. K4c must return or record
`CollapsedBorderMetricsPendingK4g` rather than approximate them with each
cell's declared borders.

The collapsed `fixed-table-layout-003d*`, `fixed-table-layout-003e*`, and
`fixed-table-layout-003f*` families remain K4g acceptance evidence. K4c still
runs and classifies them so a false pass cannot be credited to the separated
algorithm.

## Execution gates

| Gate | Outcome | Difficulty |
|---|---|---:|
| K4c1 | sizing contracts and intrinsic cell measures | 7/10 |
| K4c2 | fixed inline sizing in separated mode | 8/10 |
| K4c3 | automatic column measures | 10/10 |
| K4c4 | automatic used width and column distribution | 10/10 |
| K4c5 | parent integration, live bridge, and cleanup | 8/10 |

Accepted implementation gates land serially. The browser interop matrix for
K4c3 and K4c4 may be gathered while K4c1 and K4c2 execute. It does not select
an algorithm until its exact evidence is attached to the accepting receipt.

One task owns one gate, appends its receipt, stages only its paths, and
commits. It does not begin the next gate.

## K4c1. Sizing contracts and intrinsic cell measures

### Outcome

Give Buckram complete, logical-axis inputs for table sizing without reading a
completed layout.

### Work

- Add `components/buckram/src/table/sizing.rs` and keep table-specific types
  out of `taffy_adapter.rs`.
- Define the input, measure, result, border-metric, caption, constraint, and
  deferral types described above.
- Define an all-visible `TableTrackVisibility` input that K4f can later
  replace without forking the sizing algorithm.
- Query each cell content box through Buckram's `IntrinsicSizeQuery` contract
  for one cached min-content and max-content pair.
- Normalize computed `width`, `min-width`, `max-width`, and `box-sizing` into
  logical constraints without resolving percentages against a guessed basis.
- Add logical inline padding and separated borders to content contributions
  at the table seam.
- Compute separated-mode undistributable spacing from K4b's column count.
- Make collapsed metrics, missing caption contribution, intrinsic cycles, and
  indefinite percentage bases explicit outcomes.
- Preserve the old first-row fixed fixture as a characterization test. Do not
  wire the new algorithm to live layout yet.

### Evidence

- Pure fixtures cover content-box and border-box cells; padding; borders;
  zero-width content; min/max clamping; affine percentage constraints; both
  writing directions; and invalid intrinsic pairs.
- Query fixtures prove one min/max measurement per `(BoxId, LogicalAxis)`,
  cycle reporting, invalidation, and failure non-caching.
- Adapter fixtures prove cell measurements come from box identity and do not
  inspect Taffy nodes, grid tracks, or completed fragments.
- Fresh `css/CSS2/tables` and `css/css-tables` maps are recorded as the K4c
  baseline even if this model-only gate moves no WPT.

### Stop rules

- Stop if a measurement requires a DOM node or HTML attribute string.
- Stop if a percentage is replaced by zero, `auto`, or the current containing
  width before its algorithmic basis is known.
- Stop if border offsets are stored only as physical left and right edges.
- Stop if a failed intrinsic query enters `IntrinsicSizeCache`.

### Removal receipt

No live helper is deleted in K4c1. Record the exact new types and the old
helpers they are intended to replace.

## K4c2. Fixed inline sizing in separated mode

### Outcome

Move the fixed-layout decision and arithmetic into Buckram for tables whose
column geometry does not depend on collapsed-border winners.

### Work

- Implement the CSS 2.1 fixed precedence over the K4b grid:
  1. non-automatic column constraints;
  2. non-automatic first-row cell constraints, divided across spans;
  3. equal allocation to unresolved columns; and
  4. distribution of table space above the columns' minimum requirement.
- Apply column-group constraints over their normalized K4b column ranges.
- Include table borders, padding, and separated border spacing exactly once.
- Convert cell content-box constraints through the K4c1 offset contract.
- Keep later-row content out of column selection while preserving the
  resulting overflow for K4d.
- Make results deterministic when the available width is below the sum of
  fixed contributions.
- Make `table-layout: fixed` with `width: auto` use the automatic algorithm.
  CSS 2.1 permits this conservative policy; record it in the receipt.
- Produce logical column sizes whose order is independent of LTR or RTL.
  Physical placement remains a later fragment concern.

### Evidence

- Pure fixtures cover column versus first-row precedence; `colgroup`; spans;
  unresolved columns; insufficient width; excess width; table min-width;
  cell box sizing; spacing; subpixel remainder; and later-row invariance.
- The existing first-row definite-width fixture produces the same correct
  arithmetic through Buckram.
- Adapter fixtures prove the input is K4b's `TableGrid`.
- Focused WPT runs the complete `fixed-table-layout-*` family and classifies
  every result.
- Acceptance credit is limited to separated-mode and border-independent
  cases, including `003a*`, `003b*`, and `003c*`.
- `003d*`, `003e*`, and `003f*` remain named K4g deferrals even if the old
  compatibility path happens to render one correctly.

### Stop rules

- Stop if the column count is recovered from DOM traversal.
- Stop if a cell span is represented as a Taffy `GridPlacement`.
- Stop if later-row content changes a fixed column size.
- Stop if collapsed borders are approximated from declared cell borders.
- Stop if `width: auto` enters fixed arithmetic.

### Removal receipt

Delete the fixed arithmetic from `genet-livery::layout::fixed_column_widths`
only after the live bridge consumes Buckram's fixed result. If live wiring is
intentionally deferred to K4c5, mark the helper as a shadow assertion and
delete its decision-making branch in K4c5.

## K4c3. Automatic column measures

### Outcome

Compute the minimum, maximum, percentage, and constrained measures of every
column before choosing a used table width.

### Work

- Compute each cell's outer min-content and max-content contribution from the
  K4c1 measure and applicable computed size constraints.
- Apply single-column cells first.
- Apply column and column-group constraints to their normalized K4b ranges.
- Process spanning cells in increasing span order.
- Ensure each span can satisfy its minimum contribution and record how excess
  maximum contribution is distributed.
- Preserve percentage contributions as constraints, including percentages on
  cells, columns, and column groups.
- Bound aggregate intrinsic percentages and make competing percentage
  constraints deterministic.
- Record whether each column is constrained without conflating a specified
  percentage with a definite length.
- Select the spanning and percentage distribution rule from the interop
  matrix. The CSS Tables 3 draft's current text is a candidate description,
  not acceptance evidence.

### Evidence

- Pure fixtures cover empty columns; one-cell columns; mixed fixed and
  automatic constraints; cells spanning automatic, fixed, and percentage
  columns; nested spans; column groups; over-constrained percentages; and
  content below and above a specified width.
- Permuting unrelated cells does not change column measures.
- Reversing direction does not change logical measures.
- Focused WPT includes:
  - `computing-column-measure-*`;
  - `td-min-width-auto-layout`;
  - `td-max-width-auto-layout`;
  - `table-colspan-percent-auto`;
  - `fractional-percent-width`; and
  - `colspan-redistribution`, kept visibly tentative.
- The receipt contains the exact Chrome and Firefox interop matrix for every
  distribution choice that CSS 2.1 leaves open.

### Stop rules

- Stop if a spanning cell is handled before the columns under it have their
  lower-span measures.
- Stop if a percentage is resolved against the viewport or final table width
  during intrinsic measurement.
- Stop if a tentative WPT or the CSS Tables 3 draft is the sole reason for a
  distribution rule.
- Stop if `min_content > max_content` is repaired by swapping the values.

### Removal receipt

Delete any adapter-side cell or column intrinsic aggregation introduced by
the compatibility bridge. Buckram's `TableColumnMeasure` vector is the only
automatic column-measure source after this gate.

## K4c4. Automatic used width and column distribution

### Outcome

Choose the table's used inline size and distribute assignable space over the
K4c3 column measures.

### Work

- Derive table grid minimum and maximum widths from K4c3's column measures
  plus K4c1's undistributable space.
- Accept a minimum caption contribution as an input. A table with a caption
  whose contribution has not been measured records
  `CaptionMinPendingK4e`; it does not claim caption-sensitive acceptance.
- For a definite table width `W`, use the CSS 2.1 lower bound
  `max(W, CAPMIN, GRIDMIN)`.
- For an automatic table width with definite available size `A`, use the
  equivalent CSS 2.1 bound `max(min(GRIDMAX, A), CAPMIN, GRIDMIN)`.
- Preserve an explicit indefinite outcome when the automatic formula has no
  valid available-size basis.
- Separate used table width, used grid width, assignable column width, and
  undistributable space in the result.
- Select one interoperable column-distribution function for widths between
  the intrinsic guesses. The CSS Tables 3 four-guess interpolation model may
  be selected only if the interop receipt supports it.
- Resolve percentage constraints only when the used table basis is known.
  Report cycles and indefinite bases explicitly.
- Distribute subpixel remainders deterministically while keeping the exact
  sum invariant.
- Publish the table grid's inline `IntrinsicSizes` through Buckram's cache
  under standards-owned box identity. Wrapper intrinsic sizes remain pending
  when `CAPMIN` is pending.

### Evidence

- Pure fixtures cover available widths below the minimum, at the minimum,
  between intrinsic bounds, at the maximum, and above the maximum.
- Fixtures cover definite, automatic, percentage, min-content, max-content,
  and affine length-percentage table constraints.
- Sum assertions cover zero, one, and many columns; spacing; subpixels; and
  over-constrained percentages.
- Focused WPT includes:
  - `computing-table-width-*`;
  - `distribution-algo-*`;
  - `table-intrinsic-size-*`;
  - `min-max-size-table-content-box`;
  - percentage and colspan sizing cases; and
  - the K4c3 measure families as a regression set.
- The receipt identifies caption-sensitive cases held for K4e and
  collapsed-offset cases held for K4g.

### Stop rules

- Stop if used width is read from a completed grid or fragment.
- Stop if intrinsic size and used size are stored as one field.
- Stop if an indefinite containing width is replaced by the viewport.
- Stop if leftover pixels disappear from the table or are added twice.
- Stop if a draft-only interpolation produces the expected aggregate table
  width while assigning the wrong individual column widths.

### Removal receipt

Delete any Taffy-derived table intrinsic width. The K4c4 result becomes the
only source for table and column inline sizes, subject to the two named K4e
and K4g inputs.

## K4c5. Parent integration, live bridge, and cleanup

### Outcome

Use Buckram's sizing result in live Livery layout and leave the temporary
table bridge as a placement consumer only.

### Work

- Route fixed and automatic table inline sizing through K4c4.
- Give the compatibility bridge explicit Buckram column sizes. It may place
  the cells while K4d is pending, but it may not ask Taffy to infer the
  tracks.
- Supply table-grid intrinsic sizes, and complete wrapper intrinsic sizes
  when the caption contribution is absent or measured, to block flow, floats,
  inline tables, flex items, and grid items through Buckram's intrinsic query
  contract.
- Preserve named `CaptionMinPendingK4e`, `TrackVisibilityPendingK4f`, and
  `CollapsedBorderMetricsPendingK4g` counters in live fixtures.
- Delete Livery's `fixed_column_widths`.
- Delete table-only `horizontal_edges`, or rename and retain a genuinely
  shared logical edge helper if another live consumer is proved.
- Narrow the `table-layout` partial marker to the collapsed-border geometry
  still owned by K4g. Do not remove the marker in K4c.
- Record the exact remaining Grid/Flex compatibility bridge count. K4d owns
  its retirement; K4h audits that it remains absent at closure.

### Evidence

- Live fixtures cover a block table, inline table, float, flex item, and grid
  item at minimum, intermediate, and maximum available inline sizes.
- Fixed live fixtures prove later-row content can overflow without changing
  columns.
- Adapter counters prove all supported fixed and automatic tables received
  Buckram column sizes before entering the bridge.
- A source audit finds no table sizing decision in
  `components/genet-livery/src/layout.rs`.
- Fresh complete expectation maps for `css/CSS2/tables` and
  `css/css-tables` are compared exactly with K4b and every accepted K4c gate.
- If any other K3 ratchet directory moves, all nine directories are rerun and
  compared before acceptance.

### Stop rules

- Stop if the bridge can omit explicit columns and recover them from Taffy.
- Stop if a flex or grid parent reads a completed table rectangle for an
  intrinsic query.
- Stop if a caption or collapsed-border deferral is counted as K4c support.
- Stop if a WPT pass is credited without identifying whether Buckram or the
  compatibility path supplied the width.

### Removal receipt

The accepted K4c5 tree contains:

- no `fixed_column_widths`;
- no table-specific physical `horizontal_edges`;
- no Taffy-derived table or column width;
- one Buckram sizing path for fixed and automatic tables; and
- only the named K4e caption, K4f track-visibility, and K4g collapsed-border
  inputs still pending.

The `table-layout` partial marker remains narrowed until K4g supplies
collapsed-border metrics and reruns the collapsed fixed-layout families.

## Cross-gate dependency map

| Consumer | K4c output or input |
|---|---|
| K4d row layout | consumes final column sizes when laying out cell contents and rows |
| K4e captions | supplies `CaptionMinContribution`; consumes table and wrapper intrinsic widths |
| K4f separated rendering | supplies the track-visibility mask and uses K4c's accepted spacing and cell offsets |
| K4g collapsed borders | supplies winning logical half-border metrics, then reruns K4c sizing |
| K4h closure | verifies the K4d bridge deletion and closes positioned-table integration |
| K5 positioning | consumes table fragments and containing blocks, not K4c sizing internals |
| K6 fragmentation | may split table fragments, but does not replace intrinsic column measures |

K4g is an input completion, not a second sizing algorithm. Once winning
collapsed borders exist, K4c's fixed and automatic algorithms rerun with
different border metrics.

## Global acceptance invariants

K4c is complete when all of these are true:

1. Buckram owns every supported table and column inline-size decision.
2. Every result has exactly one finite non-negative size per K4b column.
3. Column sums, undistributable space, grid width, and table width reconcile
   within the tested subpixel rule.
4. Fixed sizing ignores later-row content.
5. Automatic sizing uses intrinsic queries and never completed layout
   rectangles.
6. Percentages and cycles remain explicit until a valid basis exists.
7. Logical column sizes are direction-neutral.
8. Collapsed-border and caption gaps have exact counters and downstream
   owners.
9. WPT movement is separated from old-bridge movement and false passes.
10. Livery contains no table sizing algorithm.

## Verification ladder for every sub-gate

1. **Model proof:** pure Buckram fixtures name the CSS distinction.
2. **Adapter proof:** computed inputs enter through box identity and logical
   constraints.
3. **Live proof:** the accepted Buckram result reaches Livery where the gate
   claims live integration.
4. **Focused corpus:** fresh exact maps cover the named WPT families.
5. **Regression ratchet:** exact status comparison against the preceding
   accepted gate, including complete CSS2 when CSS2 moves.
6. **Build proof:**

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
separately and record the exact blocker. Do not describe a partial Clippy run
as the combined command passing.

Store generated expectation maps and browser measurements under:

`C:\Users\mark_\Code\testing\genet\wpt-ledger\<date>_buckram_k4c<gate>`

Keep proof outputs out of Git.

## Receipt template

Append one receipt beneath the completed sub-gate:

```markdown
### K4cN receipt - YYYY-MM-DD

Base commit:

Capability:

Boundary retained:

Pure fixture:

Adapter fixture:

Live fixture:

Interop decision:

WPT exact movement:

Deferral counts:

Removal:

Verification:

Proof directory:

Commit:
```

## First executable task

The initial handoff is:

> Read this plan, the accepted K4b receipt, CSS 2.1 sections 17.5.2.1 and
> 17.5.2.2, and the live seams named under K4c1. Execute K4c1 only. Preserve
> unrelated worktree changes. Record the accepted K4b commit and produce
> fresh `css/CSS2/tables` and `css/css-tables` maps before changing layout
> behavior. Stop after K4c1 passes its verification ladder, append its
> receipt here, stage only K4c1 paths, and commit. Do not begin K4c2 in the
> same task.
