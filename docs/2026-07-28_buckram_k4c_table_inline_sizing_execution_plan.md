# Buckram K4c table inline sizing execution plan

**Date:** 2026-07-28

**Status:** K4c4a is complete on the accepted K4b base `26eda4cd9fe`; K4c5a is
next. K4c1 through K4c4a landed pure model code. Livery still owns every live
table sizing decision, so no WPT movement is creditable to K4c yet. K4c5 is
split into K4c5a shadow comparison and K4c5b authority and deletion.

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

### K4c1 receipt - 2026-07-29

**Base commit:** `26eda4cd9fe` (accepted K4b).

**Capability:** `buckram::table::sizing` now owns logical table-sizing
contracts: affine length-percentage constraints, table and cell box sizing,
logical inline offsets, separated and deferred-collapsed border metrics,
caption contribution, track visibility, explicit deferrals, intrinsic cell
measures, and the table input/result. `IntrinsicSizeQuery` is invoked once per
`(BoxId, LogicalAxis)` and caches only successful min/max pairs. Livery lowers
computed table and cell values into that model without inspecting Taffy nodes,
grid tracks, or completed fragments.

**Boundary retained:** the live renderer still uses its Grid/Flex compatibility
route. The explicit live fixture reports one grid for one table; the normal and
retained-inline builders are the two static compatibility construction seams.
The new sizing result is not wired into live layout. Captions (K4e), track
visibility (K4f), and collapsed borders (K4g) remain named, non-live
deferrals.

**Fixtures:** Buckram covers content and border boxes, logical padding and
borders, zero content, min/max constraints, affine percentages, LTR/RTL,
invalid pairs, cycle reporting, invalidation, and failed-query non-caching.
Livery covers physical-to-logical lowering and unresolved percentage bases.
The existing first-row fixed-width characterization remains in place.

**Interop decision:** none. K4c1 introduces the model only; it does not select
an algorithm or change table behavior.

**WPT movement:** canonical expectation-map delta from K4b is zero.
`css/CSS2/tables` remains 67 pass, 183 fail, 889 skip (1,139 total).
`css/css-tables` remains 53 pass, 77 fail, 198 skip (328 total).

**Verification:**

- `cargo test -p buckram --offline`: 91 passed.
- `cargo test -p livery --offline`: 32 passed.
- `cargo test -p genet-livery --offline`: passed, including the bridge-count
  fixture and three lowering fixtures.
- `cargo clippy -p buckram --lib --offline -- -D warnings`: passed.
- Strict all-target clippy retains two pre-existing blockers outside K4c1:
  Buckram `taffy_adapter.rs:3910` (`manual_is_multiple_of`) and Livery
  `text.rs:1399` (`implicit_saturating_sub`).
- Fresh release WPT maps are in
  `testing/genet/wpt-ledger/2026-07-29_buckram_k4c1/`; both isolated targets
  used for this gate were removed after verification.

**Removal:** no compatibility helper is deleted here. K4c5 must remove
`genet-livery::layout::fixed_column_widths` and its table-specific horizontal
edge logic only after live Buckram sizing consumes the result. K4d must remove
the Grid/Flex table bridge after the same live route is complete.

**Commit:** `8ef729852a1` (`Define Buckram table sizing contracts`).

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

### K4c2 receipt - 2026-07-29

**Base commit:** `8ef729852a1` (accepted K4c1).

**Capability:** `buckram::table::fixed` now computes separated-mode fixed
inline sizing from K4b topology and K4c1 logical constraints. It preserves
column and column-group box identities, applies explicit columns and normalized
column-group ranges before first-row cells, divides spanning first-row cell
constraints, then assigns unresolved tracks equally. Table and cell
content-box versus border-box sizes, table minimums, subpixel shares, table
padding and borders, and every separated border-spacing interval are explicit.

**Policy:** `table-layout: fixed` with `width: auto` returns the automatic
layout outcome. This is the CSS 2 permitted conservative policy. Intrinsic and
fit-content table widths take the same non-fixed outcome; an unresolved
percentage or unreduced expression remains an explicit sizing error.

**Boundary retained:** the live Grid/Flex bridge remains unchanged and does
not consume the Buckram result. The existing live fixture still records one
bridge grid for one table; there remain two static construction seams. Caption
(K4e), track visibility (K4f), and collapsed borders (K4g) return named
deferrals rather than approximations.

**Fixtures:** Buckram covers precedence, normalized colgroup ranges, first-row
spans, content and border boxes, spacing, insufficient and excess width,
table min-width, subpixel division, later-row invariance, `width: auto`, and
collapsed-border deferral. Livery proves fixed track inputs preserve K4b order
and BoxIds without DOM traversal or Taffy tracks.

**Interop decision:** none beyond the recorded `width: auto` policy. K4c2
does not wire behavior, so the CSS2 fixed-layout WPTs receive no acceptance
credit yet.

**WPT movement:** the fresh complete `css/CSS2/tables` map is canonically
unchanged from K4c1: 67 pass, 183 fail, 889 skip (1,139 total). The complete
`fixed-table-layout-*` family is also unchanged: 1 pass, 66 fail, 15 skip
(82 total). `003a` (6), `003b` (12), and `003c` (8) all remain failures on the
unwired bridge. `003d` (6), `003e` (12), and `003f` (8) remain K4g deferred
collapsed-border cases.

**Verification:**

- `cargo test -p buckram --offline`: 98 passed.
- `cargo test -p genet-livery --offline`: passed, including fixed track
  lowering.
- `cargo clippy -p buckram --lib --offline -- -D warnings`: passed.
- Strict all-target clippy retains the same external blockers: Buckram
  `taffy_adapter.rs:3910` (`manual_is_multiple_of`) and Livery `text.rs:1399`
  (`implicit_saturating_sub`).
- Fresh release WPT output is in
  `testing/genet/wpt-ledger/2026-07-29_buckram_k4c2/`; both isolated targets
  used for this gate were removed after verification.

**Removal:** no live helper is deleted. `genet-livery::layout::fixed_column_widths`
remains the compatibility-path decision maker until K4c5 consumes Buckram's
result. K4d still owns removal of the Grid/Flex bridge.

**Commit:** `03d8dce3041` (`Add Buckram fixed table sizing`).

## K4c3. Automatic column measures

**Status:** complete as a model-only gate on 2026-07-29. K4c4 is next.

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

### K4c3 receipt - 2026-07-29

**Base commit:** `03d8dce3041` (accepted K4c2).

**Capability:** `buckram::table::automatic` now computes one logical
`TableColumnMeasure` per K4b column. A measure retains min-content,
max-content, an unresolved bounded percentage, and whether a non-percentage
width constrained the track. It processes direct cells, normalized columns and
column groups, then spans in increasing span order. Every spanning cell retains
the exact min/max increment vectors it applied. No used table width, viewport
basis, Taffy track, or completed fragment enters the algorithm.

**Policy:** competing intrinsic percentages consume the remaining aggregate in
logical K4b column order. Span excess first targets unconstrained automatic
tracks and follows their existing corresponding min/max contribution, using
equal shares only when all eligible contributions are zero. A pure percentage
stays distinct from a definite constraint. The fixed-plus-percentage span
probe is a K4c4 boundary: its percentage is retained here, while selecting its
used 180px track is deferred until a table basis exists.

**Interop matrix:** Chrome `150.0.7871.187` and Firefox `153.0.1`, headless
with zero cell padding/border and zero border-spacing. Values are logical
column widths in source order. Firefox's subpixel differences are retained
instead of rounded.

| Choice | Fixture | Chrome | Firefox | K4c3 rule |
| --- | --- | --- | --- | --- |
| Competing percentages | 300px table, `60%`, `60%`, auto | `180, 120, 0` | `180, 119.983337, 0.016663` | reserve each logical percentage up to the remaining aggregate |
| Zero-weight span | `20px`, auto, auto; 300px span | `20, 140, 140` | `20, 140, 140` | equal fallback among eligible automatic tracks |
| Lower-span first | `20px`, auto, auto; 150px two-span then 300px three-span | `20, 280, 0` | `20, 280, 0` | increasing span, then existing-measure weighting |
| Existing max weighting | `20px`, auto 100px, auto 50px; 300px span | `20, 186.65625, 93.34375` | `20, 186.666672, 93.333328` | proportional maximum-excess distribution |
| Fixed plus percentage span | `20px`, `60%`, auto; 300px span | `20, 180, 100` | `20, 180, 100` | preserve the percentage for K4c4; do not resolve it intrinsically |

This matrix selected the model's deterministic rules. The CSS Tables 3 draft
was consulted only as a gap map; no tentative WPT or draft text supplies
acceptance on its own.

**Boundary retained:** Livery only lowers explicit K4b column and column-group
constraints with their `BoxId`s. It does not aggregate intrinsic values and the
live Grid/Flex compatibility table route remains unchanged. Captions (K4e),
collapsed borders (K4g), and collapsed tracks (K4f) remain named deferrals.

**Fixtures:** Buckram covers empty and one-cell grids, content above and below
specified constraints, normalized groups, nested spans, a fixed-plus-percentage
span, over-constrained percentages, logical RTL invariance, and K4f/K4g
deferrals. Livery proves automatic track lowering preserves K4b order and box
identity.

**Focused WPT:** fresh release output is in
`<workspace>\testing\genet\wpt-ledger\2026-07-29_buckram_k4c3`.
`width-distribution` is 2 all-pass / 13 with-failures, with 2/35 subtests;
`computing-column-measure-0/1/2` remain 0/9 and `td-min-width-auto-layout`
plus `td-max-width-auto-layout` remain 0/8. `table-colspan-percent-auto`
passes its one Livery reftest. `fractional-percent-width` remains 0/3 and the
explicitly tentative `colspan-redistribution` remains 0/31. This model-only
gate receives no live WPT acceptance credit.

**Verification:**

- `cargo test -p buckram`: 108 passed.
- `cargo test -p genet-livery`: passed, including automatic track lowering.
- `cargo clippy -p buckram --lib -- -D warnings`: passed.
- Strict Buckram all-target Clippy remains blocked only by the pre-existing
  `components/buckram/src/taffy_adapter.rs:3910`
  `manual_is_multiple_of` lint; it does not reach a K4c4 seam.
- Strict Buckram all-target Clippy is blocked only by the existing
  `taffy_adapter.rs:3910` `manual_is_multiple_of` warning. Strict
  `genet-livery` all-target Clippy reaches pre-existing warnings in the
  upstream `livery` crate (147 failures in the current toolchain), before
  changing this K4c3 seam.
- Both isolated Cargo targets were removed after verification. The generated
  WPT ledger remains outside Git.

**Removal:** no adapter-side intrinsic aggregation existed to delete. The new
Buckram vector is the only K4c3 automatic-measure source; the compatibility
path's fixed-width decision remains until K4c5 consumes a Buckram result.

**Commit:** `b5e47279b92` (`Add Buckram automatic column measures`).

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

### K4c4 receipt - 2026-07-29

**Base commit:** `b5e47279b92` (accepted K4c3).

**Capability:** `buckram::table::automatic_used` now selects a used automatic
table width from K4c3's logical `TableColumnMeasure` vector and returns either
a complete `TableInlineSizingResult` or a named indefinite result. The result
separates used table width, used grid width, assignable column width, and
undistributable border/padding/spacing. It derives and publishes the grid's
`IntrinsicSizes` under `TableGrid::grid` through Buckram's box-keyed cache.
Caption-sensitive wrapper work remains outside that cache.

**Policy:** a definite table width uses
`max(clamp(W, min-width, max-width), CAPMIN, GRIDMIN)`. An automatic width
with definite available inline size uses
`max(clamp(min(GRIDMAX, A), min-width, max-width), CAPMIN, GRIDMIN)`.
Intrinsic keywords and affine `fit-content()` remain CSS constraints. Missing
containing width and missing percentage bases return
`TableAutomaticInlineSizingIndefinite`; they never use a viewport fallback.
Column percentages resolve only after used table width is known. Percentage
demands share an insufficient logical remainder proportionally; then
unconstrained, non-percentage columns receive remaining room by CSS 2
`max-content - min-content` slack, and above the upper guess by max-content
weight. The final logical column receives float remainder, preserving the
sum invariant without an RTL tie-break.

**Interop matrix:** local headless Chrome `150.0.7871.187` and Firefox
`153.0.1`, with zero cell padding/border and zero border-spacing. The ordinary
three-column fixture has minima `71.1875, 71.1875, 64` in Chrome and
`71.2, 71.2, 64` in Firefox; its maxima are `146.828125, 222.453125,
269.34375` and `146.85, 222.5, 269.35`. At 300px both distribute the
min-to-max remainder proportionally to those three slacks: Chrome
`87.5625, 103.9375, 108.5`; Firefox `87.58333, 103.95001, 108.46666`.
At 400px they give `105.0625, 138.9375, 156` and
`105.08333, 138.95001, 155.96666`. A 60% first column at 300px preserves
the other minima and receives the remaining `164.8125`/`164.8` pixels. A
30% first column at 400px stays exactly 120px while the remaining two columns
receive the slack. An 80px constrained first column stays 80px. Three 60%
columns produce Chrome `135.8125, 100.171875, 64.015625` and Firefox
`135.81667, 100.18333, 64`, selecting proportional percentage demand under
over-constraint. CSS Tables Level 3's four-guess interpolation was not
adopted.

**Boundary retained:** this is pure Buckram arithmetic. It receives no Taffy
tracks, fragments, viewport substitute, or Livery layout state. K4c5 alone
may route `size_automatic_table_inline` into the live compatibility bridge.
K4e caption measurement, K4f collapsed-track handling, and K4g collapsed
border winners remain named deferrals.

**Fixtures:** ten K4c4 Buckram tests cover automatic widths below, at,
between, and above intrinsic bounds; definite widths; min/max, captions,
min-content, max-content, and affine fit-content constraints; indefinite and
percentage bases; percentage and constrained tracks; over-constrained
percentages; empty, single, and many-column sums; separated geometry,
subpixels, cache identity, caption deferral, and LTR/RTL invariance.

**Focused WPT:** fresh release output is in
`<workspace>\testing\genet\wpt-ledger\2026-07-29_buckram_k4c4`.
`width-distribution` remains 2 all-pass / 13 with-failures, 2/35 subtests;
`fractional-percent-width` remains 0/3 and tentative
`colspan-redistribution` remains 0/31. The Livery
`table-colspan-percent-auto` reftest remains its one pass. The four
`table-intrinsic-size-*` reftests and
`min-max-size-table-content-box` remain local failures. This is unchanged
compatibility-path evidence, not live K4c4 acceptance credit.

**Verification:**

- `cargo test -p buckram --lib`: 118 passed.
- `cargo clippy -p buckram --lib -- -D warnings`: passed.
- `cargo build -p genet-wpt --release --all-features --offline`: passed,
  with pre-existing warnings in unrelated upstream crates.
- `cargo test -p genet-livery`: passed (95 tests across unit and integration
  targets, plus doc-tests).
- `git diff --check` and touched-file Rustfmt: passed.

**Removal:** no live Livery helper is removed in this model-only gate. K4c5
owns deletion of the compatibility bridge's table-width choices after it
consumes this result.

**Commit:** `acba7c84268` (`Add Buckram automatic table used sizing`).

## K4c4a. Padding percentage retention

### Outcome

`CellInlineOffsets` carries a padding percentage to Buckram instead of
rejecting it during style lowering.

### Why this precedes K4c5

K4c1 lowered padding to `f32` and returned `UnresolvedPercentageBasis` for any
percentage. Livery's live `horizontal_edges` resolves one, and
`fixed_column_widths` gives it the table's content width. K4c5 deletes both.
Deleting a live helper that handles a case the model rejects would regress
every fixed table with percentage cell padding, and no K4c5 stop rule catches
it, because `TableInlineSizingError` has no live consumer.

This also brought the contract into line with global invariant 6. Fixed layout
establishes its table width before distributing columns, so a basis genuinely
exists there; only automatic layout is circular.

### Work

- `CellInlineOffsets::padding_start` and `padding_end` became
  `AffineLengthPercentage`. Borders stay absolute, because CSS has no
  percentage border width.
- `total` takes a percentage basis. `absolute_total` returns `None` rather than
  sampling a percentage at zero.
- `TableInlineSizingInput::table_padding_basis` is the single source of the
  table box's own basis, so the two `separated_metrics` copies cannot diverge.
- Fixed cell sizing resolves against the requested table size, which is the
  basis its width constraint already used.
- Automatic measures return the new `TableDeferral::PercentagePaddingPendingBasis`.
- Livery's `logical_padding` returns the affine value and picks no basis.

### Boundary retained

Livery lowers and does not resolve. Every basis is chosen inside Buckram.

### Deferral counts

Automatic tables with percentage cell padding defer under
`PercentagePaddingPendingBasis` and stay on the compatibility bridge until a
gate supplies a post-measure basis. This is a named deferral, not a silent
fallback.

### Verification

- `cargo test -p buckram --lib`: 120 passed, 0 failed (118 before this gate).
- `cargo test -p genet-livery`: all targets passed, 0 failed.
- `cargo clippy -p buckram --offline --no-deps -- -D warnings`: passed.
  The combined command is blocked by a pre-existing unrelated warning in
  `components/genet-livery/src/text.rs:1399`, which this gate does not touch.
- Rustfmt on touched files and `git diff --check`: passed.

### Known divergence for K4c5a to compare

Buckram resolves a cell percentage against `requested_table_size`; Livery
resolves against the table's *content* width. These agree for a content-box
table, and differ for a border-box table with table padding. K4c2 already had
this divergence for cell width percentages, so it is not introduced here. The
shadow comparison must classify it before K4c5b deletes the live helper.

**Commit:** `e9fedff78a5` (`Carry table padding percentages into Buckram`).

## K4c5a. Shadow integration

### Outcome

Buckram's result is computed for every live table and compared against the
existing Livery path. Nothing is deleted and no live geometry changes.

### Work

- Lower live table inputs once and call Buckram for fixed and automatic tables.
- Keep `fixed_column_widths` authoritative for painted output.
- Assert agreement per table between the Buckram result and the live path, and
  log divergence with the table's box identity and the disagreeing quantity.
- Count, and do not silently absorb, every `TableDeferral` reached live.

### Progress

The fixed path is wired and live. `components/genet-livery/src/table_shadow.rs`
lowers each live table once, runs Buckram beside `fixed_column_widths`, and
records disagreements against the table's `BoxId`. The ledger is reachable as
`LiveryLayout::table_shadow_ledger`. Painted output is unchanged.

#### First classified divergence: Livery omits `border-spacing`

Buckram is the correct side. CSS 2.1 17.5.2.1 shares remaining table space over
the columns after borders and spacing; Livery's fixed algorithm never subtracts
`border-spacing`, which its own `table-layout` partial marker already admits.

With the UA sheet's `border-spacing: 2px` and `td { padding: 1px }`, a 300px
fixed table whose first of three cells is 120px gives:

| | Column 0 | Columns 1 and 2 |
|---|---:|---:|
| Buckram | 122 | 85 |
| Livery | 122 | 89 |

Livery distributes `(300 - 122) / 2`; Buckram distributes
`(300 - 8 - 122) / 2`, where 8px is four gaps of 2px. Every live table fixture
in `components/genet-livery/tests/anonymous_boxes.rs` sets `border-spacing: 0`,
which is why this never surfaced. **Accepted rule:** K4c5b takes Buckram's
value, so this divergence is expected to persist until the live helper is
deleted. It is a live bug that K4c5b fixes, not a Buckram defect.

The control test pins the other half: with `border-spacing: 0` the two
implementations agree exactly.

#### Named gaps before K4c5a can be accepted

Neither is silent, and neither may be counted as support.

1. **Atomic-subtree ledgers are discarded.** `layout_atomic_subtrees` builds one
   `BuildState` per atomic root inside a loop and drops each one, exactly as it
   already drops `table_bridge_count`. Tables laid out through the text path
   therefore report an empty ledger. They must be accumulated through
   `AtomicLayoutPlane` before this gate closes.
2. **Automatic tables are not compared.** Buckram's automatic algorithm needs a
   `TableIntrinsicMeasureProvider`, and intrinsic content sizes do not exist at
   box-build time, so the comparison has no partner. The live path also has no
   column vector for an automatic table, since `fixed_column_widths` returns
   `None`. Wiring the provider is the remaining half of this gate.

#### Live root font size

`length_percentage_px` and `border_width_px` resolve `rem` against a hardcoded
16px rather than the root element's computed font size. The shadow matches that
constant deliberately, or every `rem` table would report an artifact. Fixing the
live assumption is its own change, and K4c5b must not inherit it silently.

### Evidence

- A divergence ledger over the `css/CSS2/tables` and `css/css-tables` corpora
  naming every disagreeing table and quantity.
- The border-box table padding divergence above is either resolved or
  explicitly accepted with a recorded rule.
- No expectation-map movement, because no live behavior changed. Any movement
  is a bug in this gate.

### Stop rules

- Stop if a divergence is silenced rather than classified.
- Stop if the shadow path changes a painted result.

## K4c5b. Authority, live bridge, and cleanup

### Entry gate

K4c5a's shadow comparison is silent, or every remaining divergence has a
recorded and accepted rule.

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

`<workspace>\testing\genet\wpt-ledger\<date>_buckram_k4c<gate>`

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

## Next executable task

K4c1 through K4c4a are accepted. The current handoff is:

> Read this plan, the accepted K4c4a receipt, CSS 2.1 sections 17.5.2.1 and
> 17.5.2.2, and the live seams named under K4c5a. Execute K4c5a only. Preserve
> unrelated worktree changes. Compute Buckram's result for every live table
> and compare it against the existing Livery path without changing painted
> output or deleting anything. Produce a divergence ledger naming each
> disagreeing table and quantity, and classify the border-box table padding
> divergence recorded under K4c4a. Expectation maps must not move. Stop after
> K4c5a passes its verification ladder, append its receipt here, stage only
> K4c5a paths, and commit. Do not begin K4c5b in the same task.

K4c5 is the first gate that changes live behavior, which is why it is split.
K4c5a proves Buckram agrees with the live path while both exist; K4c5b makes
Buckram authoritative and deletes the old path. Landing both at once would mix
any regression with the removal of the code that would have revealed it.
