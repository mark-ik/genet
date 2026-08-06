# Buckram K4g collapsed border execution plan

**Date:** 2026-07-28

**Status:** scoped; execution starts after the accepted K4f receipt

**Parent plan:** [Buckram K4 CSS tables execution plan](2026-07-28_buckram_k4_css_tables_execution_plan.md)

**Sizing predecessor:** [Buckram K4c table inline sizing execution plan](2026-07-28_buckram_k4c_table_inline_sizing_execution_plan.md)

**Row-layout predecessor:** [Buckram K4d table row layout execution plan](2026-07-28_buckram_k4d_table_row_layout_execution_plan.md)

**Architectural authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

## Ruling

K4g resolves one collapsed border grid before table sizing and paints that
resolved grid once.

The algorithm consumes K4b topology, computed border candidates for every
table role, K4f visibility state, and the table's logical flow. It resolves
winning border segments without layout rectangles. Those winners produce
block and inline metrics for K4c and K4d. Final column and row geometry then
turns the same winners into paint segments.

Conflict resolution, sizing metrics, geometry, and paint share one resolved
border identity. Paint does not rerun the conflict algorithm. Table sizing
does not inspect painted widths. Generic per-cell borders do not remain
active beneath the resolved grid.

## Entry gate

Execution starts from an accepted K4f commit and receipt.

The accepted predecessor chain must provide:

- K4b's normalized row and column grid, spans, groups, and stable box order;
- K4c's explicit `CollapsedBorderMetricsPendingK4g` sizing input;
- K4d's table dispatcher, row-layout result, fragments, and explicit
  `CollapsedBlockBorderMetricsPendingK4g` input;
- K4e's wrapper and grid distinction;
- K4f's table background phases, separated-border rendering, empty-cell
  behavior, and visibility-collapse track state; and
- one standards-owned `FragmentId` for every table role that can own paint or
  overflow.

K4g freezes the accepted K4f commit and produces fresh expectation maps for:

- `css/CSS2/tables`;
- `css/css-tables`;
- `css/css-writing-modes`; and
- every other K3 ratchet directory moved by K4f's paint integration.

## Standards boundary

[CSS 2.1 section 17.6.2](https://www.w3.org/TR/CSS2/tables.html#collapsing-borders)
is stable authority for the collapsed model:

- borders are centered on table grid lines;
- a row width contains full interior winning borders and half of each outer
  winning border;
- later rows with larger outer inline borders may spill into the table's
  margin area;
- outer block-start and block-end borders use half of the maximum collapsed
  border meeting those table edges;
- collapsed-border spill contributes to overflow; and
- table padding does not participate in the collapsed model.

[CSS 2.1 section 17.6.2.1](https://www.w3.org/TR/CSS2/tables.html#border-conflict-resolution)
defines the conflict precedence:

1. `hidden` suppresses every conflicting border at that location;
2. `none` has the lowest priority;
3. wider borders beat narrower borders;
4. equal-width styles rank `double`, `solid`, `dashed`, `dotted`, `ridge`,
   `outset`, `groove`, then `inset`;
5. equal remaining candidates rank cell, row, row group, column, column
   group, then table; and
6. equal-origin ties use table direction and source position.

CSS 2.1 also maps collapsed `inset` to `ridge` and `outset` to `groove`.

Several geometry and rendering details are not sufficiently interoperable in
CSS 2.1:

- how one spanning side is harmonized when it meets multiple shorter sides;
- how borders meet at multi-way intersections and corners;
- the exact ownership and clipping used to paint a winning segment;
- odd-device-pixel allocation;
- vertical and sideways writing-mode tie behavior; and
- dynamic removal and visibility-collapse interactions.

The current [CSS Tables Level 3 collapsed-border section](https://drafts.csswg.org/css-tables-3/#border-collapsing)
calls itself a proposal and says browser implementations visibly diverge.
Its harmonization, clipping, and paint rules are interoperability candidates,
not standalone standards receipts.

Every non-CSS2 choice requires an interop record containing:

- the exact WPT or reduced fixture;
- Chrome and Firefox build versions and device scale;
- the candidate set and selected winner for each disputed segment;
- the measured table, cell, and overflow geometry;
- the observed paint ownership and pixel allocation; and
- the rule selected for Buckram.

Tentative WPT remains labeled tentative even when it matches the selected
rule.

## Live debt at the entry seam

Before K4g, the live path has borders but no collapsed-border model:

- computed styles expose physical border style, width, and color per box side;
- generic Livery paint emits one normal rectangular border per fragment;
- no table border candidate grid or conflict resolver exists;
- no border winner feeds K4c intrinsic or used inline sizing;
- no border winner feeds K4d row sizing;
- table overflow cannot include later-row outer border spill;
- `DrawBorder` assumes one rectangular box owns all four sides; and
- the generic DOM paint walk can draw the same shared table edge more than
  once.

The neutral paint list also provides filled and stroked path primitives. K4g
first proves whether those primitives can express collapsed segments, joins,
double lines, dashes, dots, and relief styles. It extends
`paint_list_api` only if an image receipt proves the existing primitives
cannot represent an accepted result.

## Border-grid contracts

The exact Rust spelling may change during K4g1. These distinctions may not.

```rust
pub enum TableBorderOrigin {
    Cell,
    Row,
    RowGroup,
    Column,
    ColumnGroup,
    Table,
}

pub struct TableGridEdge {
    pub orientation: GridEdgeOrientation,
    pub line: usize,
    pub segment: usize,
}

pub struct TableBorderCandidate<Color> {
    pub edge: TableGridEdge,
    pub source: BoxId,
    pub source_side: PhysicalSide,
    pub origin: TableBorderOrigin,
    pub style: TableBorderStyle,
    pub width: f32,
    pub color: Color,
    pub order: TableBorderOrderKey,
}

pub struct ResolvedTableBorder<Color> {
    pub edge: TableGridEdge,
    pub winner: Option<BoxId>,
    pub style: TableBorderStyle,
    pub width: f32,
    pub color: Option<Color>,
    pub suppressed_by_hidden: bool,
}

pub struct ResolvedTableBorderGrid<Color> {
    pub segments: Vec<ResolvedTableBorder<Color>>,
}

pub struct CollapsedBorderMetrics {
    pub cell_offsets: Vec<CellCollapsedBorderMetrics>,
    pub table_outer: LogicalSides<f32>,
    pub overflow: LogicalSides<f32>,
}

pub struct CollapsedBorderGeometry<Color> {
    pub segments: Vec<CollapsedBorderPaintSegment<Color>>,
    pub overflow: LogicalRect,
}
```

The color payload stays generic or opaque to Buckram. Conflict resolution
compares origin, style, width, and order. It carries the winning computed
color without importing Livery or paint-list types.

The contracts preserve these invariants:

1. `TableGridEdge` identifies one atomic segment between adjacent grid
   intersections.
2. A candidate may cover several atomic segments, but each segment resolves
   independently until an accepted harmonization rule says otherwise.
3. `hidden` remains observable as suppression even though the resolved painted
   width is zero.
4. `none` remains distinct from an absent candidate during diagnostics.
5. Winner identity and source side survive into metrics and paint.
6. Physical computed border sides are mapped into the table's logical grid
   before conflict comparison.
7. Atomic segment winners remain available after adjacent equal segments are
   coalesced for paint.
8. Layout widths stay in CSS pixels. Device-pixel snapping is a named paint
   decision.
9. A spanning cell side with multiple segment winners is not reduced to an
   arbitrary first or maximum width.
10. Every finite non-negative winning width appears in sizing, geometry, and
    paint exactly once under the selected model.

## Spanning-side metric seam

A scalar `LogicalSides<f32>` is insufficient as the primary collapsed-border
model. One side of a spanning cell can meet several cells and therefore
several atomic conflict sets.

`CellCollapsedBorderMetrics` must retain either:

- the ordered winning segments along each side; or
- a scalar side value plus the exact accepted harmonization proof that makes
  every segment on that side equivalent.

K4c and K4d receive a projected intrinsic or used offset only after K4g3 has
selected that projection from current interoperability evidence. They must
not choose `first`, `last`, `maximum`, or `average` on their own.

## Resolution and layout sequence

The accepted data flow is:

1. build atomic grid-edge topology from K4b;
2. collect candidates from computed table styles;
3. resolve CSS2 precedence per atomic segment;
4. apply only the accepted spanning-side harmonization;
5. derive collapsed inline and block metrics;
6. rerun K4c with collapsed inline metrics;
7. rerun K4d with collapsed block metrics;
8. generate final segment and corner geometry from row and column positions;
9. transform logical geometry to physical geometry; and
10. emit the resolved border grid once in K4f's table paint phases.

Winner selection cannot depend on final segment length. Paint cannot change a
winner selected before sizing.

## Execution gates

| Gate | Outcome | Difficulty |
|---|---|---:|
| K4g1 | atomic border topology and candidate extraction | 8/10 |
| K4g2 | CSS2 conflict precedence per segment | 9/10 |
| K4g3 | spanning-side harmonization and metric projection | 10/10 |
| K4g4 | K4c/K4d sizing and overflow integration | 10/10 |
| K4g5 | final border geometry, styles, and paint phase | 10/10 |
| K4g6 | dynamic integration, cleanup, and closure | 9/10 |

Accepted implementation gates land serially. The K4g3 and K4g5 browser and
image matrices may be gathered while K4g1 and K4g2 execute. They do not
select an algorithm until their evidence is attached to the accepting
receipt.

One task owns one gate, appends its receipt, stages only its paths, and
commits. It does not begin the next gate.

## K4g1. Atomic border topology and candidate extraction

### Outcome

Represent every border that can meet at every atomic table grid edge without
performing conflict resolution.

### Work

- Add or complete `components/buckram/src/table/borders.rs`.
- Define the candidate, origin, edge, order, resolved-grid, metric, geometry,
  and deferral types described above.
- Build atomic inline-running and block-running edge segments from K4b row and
  column intersections.
- Project cell borders over only the perimeter of their normalized spans.
- Project row, row-group, column, column-group, and table borders over the
  exact track ranges where CSS2 permits them to participate.
- Preserve anonymous-box provenance and source order.
- Map Livery's physical computed sides into the table grid's logical axes at
  the adapter boundary.
- Carry style, finite used width, computed color, origin role, source
  identity, source side, table direction, and stable order.
- Preserve `hidden` and `none` candidates.
- Consume K4f's visibility-collapse state without deleting track topology.
- Produce a candidate ledger for reduced fixtures before selecting winners.

### Evidence

- Pure fixtures cover every table origin role, all four sides, outer and
  interior edges, rowspans, colspans, missing cells, anonymous groups, LTR,
  RTL, vertical writing, and collapsed tracks.
- Every candidate maps to at least one valid atomic segment.
- Candidate collection is invariant under paint traversal order.
- Adapter fixtures prove computed physical sides are mapped once at the
  logical table boundary.
- Focused WPT is baselined for CSS2 `border-conflict-*`,
  `collapsing-border-model-*`, `bidi-border-collapse-*`, css-tables
  `border-conflict-resolution`, rowspan collapse, writing-mode collapse, and
  dynamic collapse families.

### Stop rules

- Stop if candidate extraction reads DOM tags or HTML attributes.
- Stop if one rectangle is reconstructed from cell fragments to infer row or
  column edges.
- Stop if `hidden` is discarded because its used painted width is zero.
- Stop if a spanning side is represented as one edge before the K4g3
  harmonization decision.
- Stop if visibility collapse deletes K4b topology.

### Removal receipt

No generic border painting is deleted in K4g1. Record the exact candidate
sources and edge counts for every pure and live fixture.

### K4g1 receipt - 2026-08-06

Base commit: `bad53b5cb2f`.

**Capability:** `components/buckram/src/table/borders.rs` answers one question
- *which borders meet here* - for every atomic segment of a table grid, and
answers nothing else. No winner is selected, no geometry is consulted, and no
paint changes.

**The contracts landed as the plan specified them**, with two spellings
settled by writing them down. `TableBorderOrigin` and `TableBorderStyle` are
declared in CSS 2.1 section 17.6.2's own precedence order, so their derived
`Ord` *is* the precedence relation and K4g2 compares them directly rather than
restating the table. `TableBorderSide` is retained on every candidate, because
a segment's winner being one row's block-end rather than the next row's
block-start survives into paint even where the two land on one line.

**Invariants held by construction rather than by assertion:**

- A candidate covering several atomic segments becomes several candidates, one
  per segment. A spanning cell's side is never one edge, which is the stop rule
  the plan states about spanning sides, and the fixture reads the count.
- `hidden` and `none` are collected. A resolved `hidden` paints nothing, but
  "suppressed here" and "no candidate here" are different answers and K4g2 has
  to tell them apart.
- Collection order is fixed by role - table, groups, tracks, cells - so the
  ledger is invariant under paint traversal, which the plan requires and which
  a role-ordered loop gives for free.
- K4f's collapsed tracks keep their intersections and their candidates. Removing
  them here would delete the model rather than the rendering.
- A negative or non-finite width is an error rather than a clamp: a clamped
  width would enter sizing as a real zero-width border.

**Colour stays generic.** `TableBorderCandidate<Color>` never inspects its
payload; resolution compares style, width, origin, and order, and carries the
winner's colour through. That is what keeps Livery's colour model out of the
table model without an opaque handle.

**Pure fixture:** seven in `borders.rs` - interior-line coverage from both
sides, a spanning side's per-track segments, `hidden`/`none` survival, a row
group projecting over exactly its range and never onto an interior line, a
collapsed track keeping its edges, a rejected negative width, and the
precedence order being the enums' own order.

**Adapter fixture:** none. K4g1 has no adapter surface yet - Livery lowers
nothing into these types until K4g2 needs winners.

**WPT:** unchanged by design. The plan's removal receipt for this gate says no
generic border painting is deleted here, and nothing that renders today takes
a different route.

**Verification:** 528 tests across the three crates (buckram 172), 0 failed.
`cargo clippy -p buckram` clean. Rustfmt and `git diff --check` clean.

**Not yet done in K4g1:** the adapter mapping from Livery's physical computed
sides into the grid's logical axes, which the plan lists here but which has no
consumer until K4g2 - writing it now would be a lowering with nothing to
lower into. `TableBorderOrderKey` is defined but the direction-corrected index
that fills it is the adapter's, and unwritten. Vertical writing modes are
representable and untested.

## K4g2. CSS2 conflict precedence per segment

### Outcome

Resolve one CSS2 winner for each atomic segment without using layout geometry
or paint order.

### Work

- Implement the CSS2 precedence as one pure comparator:
  1. `hidden`;
  2. `none`;
  3. width;
  4. style rank;
  5. origin rank; and
  6. direction-aware source position.
- Resolve `hidden` to an explicit suppressed segment.
- Resolve an all-`none` set to an omitted segment.
- Preserve the original winning style for diagnostics while mapping
  collapsed `inset` to `ridge` and `outset` to `groove` for paint.
- Derive direction-aware ties from K4b grid order and the table's flow, not
  DOM traversal or physical left alone.
- Keep color as a carried winner value. Color value itself does not rank.
- Make the comparator total and deterministic for anonymous and equal-source
  candidates.
- Add a winner ledger that reports every candidate and the exact comparison
  step that eliminated it.

### Evidence

- Pure pairwise fixtures cover every precedence boundary and demonstrate
  comparator antisymmetry and transitivity.
- Permuting the input candidate vector does not change the winner.
- LTR and RTL fixtures reverse only the CSS-defined positional tiebreak.
- Focused WPT includes:
  - CSS2 `border-conflict-style-*`;
  - `border-conflict-width-*`;
  - `border-conflict-element-*`;
  - `border-conflict-resolution-*`;
  - `border-conflict-example-*`; and
  - hidden and none cases from the collapsing-border families.
- A source audit finds one conflict comparator.

### Stop rules

- Stop if the widest border wins before `hidden` is checked.
- Stop if style rank is delegated to enum declaration order.
- Stop if row wins over cell or column wins over row group.
- Stop if paint order breaks an otherwise equal conflict.
- Stop if physical left and top are used without the table's direction and
  writing mode.

### Removal receipt

Delete any duplicate border-precedence helper introduced by paint or sizing.
The pure K4g2 comparator is the only winner selector.

## K4g3. Spanning-side harmonization and metric projection

### Outcome

Resolve connected spanning-side conflicts and derive the border metrics K4c
and K4d can safely consume.

### Work

- Build the Chrome and Firefox matrix for a spanning side meeting multiple
  shorter sides with different widths, styles, origins, and colors.
- Compare per-segment CSS2 resolution with the current CSS Tables
  harmonization proposal and stable WPT expectations.
- Select and record the accepted connected-set or per-segment rule.
- Keep atomic winners even if the accepted rule harmonizes their used values.
- Define how a piecewise winning side contributes to:
  - a cell's intrinsic inline-start and inline-end offsets;
  - its used block-start and block-end offsets;
  - outer table edges;
  - row and column track geometry; and
  - overflow.
- Return explicit `CollapsedBorderMetrics` for K4c and K4d.
- Preserve half-width values as CSS-pixel geometry without device rounding.
- Keep border radius, border image, and corner paint out of metric selection.

### Evidence

- Pure fixtures cover one-to-one edges, one spanning side against two or more
  neighbors, nested rowspans and colspans, missing cells, group boundaries,
  outer edges, and mixed winning widths along a side.
- Projected cell and table offsets can be traced back to exact atomic winners.
- No projection silently selects the first, last, maximum, or average segment.
- Focused WPT includes `border-collapse-rowspan-cell`,
  `rowspan-cell-border-after-color`, collapsed spanning-cell families,
  `chrome-rowspan-bug`, and current writing-mode span cases.
- The receipt contains the exact harmonization matrix and identifies any
  stable interop split that remains a named deferral.

### Stop rules

- Stop if K4c or K4d independently derives collapsed offsets.
- Stop if adjacent equal paint segments are coalesced before candidate and
  winner diagnostics are retained.
- Stop if CSS Tables 3 proposal text is the sole harmonization evidence.
- Stop if device-pixel rounding changes a layout metric.
- Stop if a true stable browser split is described as full conformance.

### Removal receipt

Delete `CollapsedBorderMetricsPendingK4g` and
`CollapsedBlockBorderMetricsPendingK4g` only for the cases covered by the
accepted projection. Any surviving interoperability split keeps a named,
counted reason.

## K4g4. K4c/K4d sizing and overflow integration

### Outcome

Run the existing Buckram column and row algorithms with collapsed metrics and
produce final table geometry.

### Work

- Feed K4g3 inline metrics into the K4c fixed and automatic sizing paths.
- Feed K4g3 block metrics into the K4d row-layout path.
- Ignore table border spacing in collapsed mode.
- Apply the accepted collapsed-table padding rule rather than treating
  collapse as separated mode with zero spacing.
- Include full interior winner widths and the accepted half outer widths
  exactly once.
- Compute logical row and column grid lines from the rerun K4c and K4d
  results.
- Compute outer border spill, including a later row whose outer inline winner
  exceeds the first row's table edge contribution.
- Union border spill into table and ancestor overflow.
- Keep fixed-layout later-row content out of column selection while allowing
  its collapsed border to affect overflow where CSS2 requires.
- Rerun automatic sizing cases whose intrinsic offsets changed.

### Evidence

- Sum fixtures reconcile content, padding, full interior borders, half outer
  borders, and used table size for every row and column.
- Pure fixtures cover different outer winners by row, different block-edge
  winners by column, odd CSS-pixel widths, spans, and empty tables.
- Focused WPT includes:
  - `fixed-table-layout-003d*`, `003e*`, and `003f*`;
  - `collapsing-border-model-*`;
  - `border-collapse-offset-*`;
  - `border-collapse-empty-row`;
  - `subpixel-table-cell-width-*`;
  - collapsed intrinsic and automatic sizing cases; and
  - collapsed writing-mode overflow families.
- K4c and K4d regression fixtures rerun under both border models.

### Stop rules

- Stop if K4c or K4d forks into a second collapsed-only sizing algorithm.
- Stop if table padding or border spacing is carried over accidentally from
  separated mode.
- Stop if an outer spill is clipped to the table border box before overflow
  propagation.
- Stop if later-row content changes fixed columns.
- Stop if layout widths are snapped to device pixels.

### Removal receipt

Delete the collapsed sizing deferrals for accepted cases. Remove every
fallback that models collapse as separated borders with zero spacing.

## K4g5. Final border geometry, styles, and paint phase

### Outcome

Convert the resolved border grid to final logical and physical paint geometry
and emit it once in table paint order.

### Work

- Generate segment endpoints from final K4g4 row and column grid lines.
- Generate intersection and corner geometry without rerunning conflict
  precedence.
- Keep logical segment geometry until the final table-flow transform.
- Select and record the accepted rule for odd-device-pixel ownership and
  multi-way joins.
- Render `solid`, `double`, `dashed`, `dotted`, `ridge`, `groove`, and the
  collapsed mappings of `inset` and `outset`.
- Omit suppressed and all-`none` segments.
- Ignore border radius in collapsed mode under the accepted standards and
  interop rule.
- Attach winner `BoxId` and table `FragmentId` provenance to the internal
  paint segment.
- Emit collapsed borders in K4f's table phase, after the relevant backgrounds
  and in the accepted relation to cell contents and positioned descendants.
- Suppress generic table and cell `DrawBorder` emission in collapsed mode.
- Prove existing neutral `DrawPath`, `DrawStroke`, and `DrawBorder`
  primitives can express every accepted style and join. If they cannot,
  stop and split this gate into:
  - a neutral `paint_list_api` provider change with renderer receipts; and
  - a Genet consumer change with table image receipts.

### Evidence

- Pure geometry fixtures cover outer edges, interior crossings, T-junctions,
  spans, unequal neighboring widths, double lines, dashes, dots, relief
  styles, and transparent colors.
- Image fixtures run at device scales 1 and 2 and compare LTR, RTL,
  horizontal, vertical-lr, vertical-rl, and sideways cases.
- Command audits prove each winning segment is emitted once and suppressed
  segments are absent.
- Focused WPT includes:
  - `collapsed-border-paint-phase-*`;
  - `subpixel-collapsed-borders-*`;
  - `collapsed-border-writing-mode-color`;
  - `out-of-order-elements-collapsed-border`;
  - CSS2 collapsed-border style families; and
  - the tentative paint-order family, still labeled tentative.

### Stop rules

- Stop if one normal rectangular border is drawn for every cell.
- Stop if paint chooses a different winner than the resolved grid.
- Stop if equal layout geometry produces different paint solely because DOM
  traversal order changed.
- Stop if CSS-pixel layout values are permanently replaced by device-rounded
  values.
- Stop if a neutral paint API change is hidden inside the Genet consumer
  commit.

### Removal receipt

Delete generic collapsed table and cell border emission. The resolved border
grid becomes the only source of collapsed border paint.

## K4g6. Dynamic integration, cleanup, and closure

### Outcome

Keep layout and paint correct across style and structure changes, remove the
remaining collapsed-border fallbacks, and close K4g.

### Work

- Recompute the candidate and resolved grids when a participating border
  style, width, color, origin role, span, direction, writing mode,
  visibility, row, column, or group changes.
- Classify a color-only winner change separately from a winner-width change.
  A full table rebuild is acceptable when recorded; do not claim incremental
  invalidation unless it is proved.
- Integrate K4f visibility-collapse behavior with candidate participation,
  geometry, and paint.
- Handle cell, row, group, and column removal without retaining stale winners
  or segment geometry.
- Preserve positioned collapsed-border cases on Buckram's table path.
  K4h owns relative offsets and the K5 positioning routes.
- Preserve multicolumn, pagination, repeated-header, and split-rowspan cases
  as K6 fragmentation work.
- Delete all collapsed-as-separated fallbacks, duplicate paint decisions,
  temporary diagnostics, and accepted deferral variants.
- Remove the `table-layout` partial marker after collapsed metrics have passed
  K4c fixed and automatic sizing receipts.
- Record any global `border-image` limitation under the existing unimplemented
  border-image property rather than presenting it as K4g support.

### Evidence

- Live mutation fixtures change every candidate field and remove cells, rows,
  groups, and columns.
- Winner-width changes rerun K4c and K4d; color-only changes preserve geometry
  while repainting the winning segment.
- Source and command audits find one resolved border grid, one metrics
  projection, and one paint lowering.
- Fresh complete maps for `css/CSS2/tables`, `css/css-tables`, and
  `css/css-writing-modes` are compared with K4f and every accepted K4g gate.
- Complete all-nine maps are compared because shared sizing, fragments, and
  paint moved.
- `collapsed-border-positioned-tr-td` and position-sensitive CSS2 cases are
  classified for K4h rather than credited prematurely.

### Stop rules

- Stop if a mutation can leave paint using an older winner grid than layout.
- Stop if a color change silently triggers a different geometry rule.
- Stop if a removed table part retains a candidate through stale box
  identity.
- Stop if positioned or fragmented false passes are credited to K4g.
- Stop if the `table-layout` marker is removed before the collapsed K4c
  rerun.

### Removal receipt

The accepted K4g6 tree contains:

- one candidate grid;
- one CSS2 conflict comparator;
- one accepted spanning-side projection;
- one metrics path into K4c and K4d;
- one final geometry path;
- one collapsed border paint path;
- no generic per-cell collapsed border paint; and
- no collapsed-as-separated sizing or paint fallback.

Only named K4h positioning and K6 fragmentation cases remain outside the
accepted unfragmented collapsed-border model.

## Cross-gate dependency map

| Consumer | K4g output or input |
|---|---|
| K4b topology | supplies atomic row, column, group, span, and source-order ranges |
| K4c sizing | consumes collapsed inline metrics and returns final column sizes |
| K4d rows | consumes collapsed block metrics and returns final row sizes |
| K4e wrapper | owns outer flow and caption geometry around the resolved table grid |
| K4f rendering | supplies background phases and visibility state; receives resolved border paint |
| K4h positioned tables | applies relative offsets and audits position-sensitive collapsed cases |
| K5 positioning | consumes final table containing fragments |
| K6 fragmentation | splits and repeats resolved border geometry across fragmentainers |

## Global acceptance invariants

K4g is complete when all of these are true:

1. Every atomic grid segment has a deterministic resolved outcome.
2. CSS2 conflict precedence has one pure implementation.
3. Spanning-side projection has current interoperability evidence.
4. K4c and K4d consume winner-derived metrics.
5. Layout values remain in CSS pixels until paint.
6. Outer border spill contributes to overflow.
7. Paint uses the same winner identity as sizing.
8. Every winning segment is painted once.
9. Dynamic changes cannot leave stale winners or geometry.
10. Positioning and fragmentation gaps remain explicitly routed.

## Verification ladder for every sub-gate

1. **Model proof:** pure border-grid fixtures name the CSS distinction.
2. **Adapter proof:** computed borders become logical candidates without DOM
   or paint-list types entering Buckram.
3. **Sizing proof:** K4c and K4d receive exact winner-derived metrics.
4. **Live proof:** fragments, overflow, commands, and mutation counters show
   the accepted behavior.
5. **Image proof:** device-scale and writing-mode images cover paint geometry.
6. **Focused corpus:** fresh exact maps cover the named WPT families.
7. **Regression ratchet:** exact comparison against the preceding accepted
   gate, complete CSS2, and all nine when shared code moves.
8. **Interop receipt:** every proposal-derived rule records current browser
   and WPT evidence.
9. **Build proof:**

   ```powershell
   $env:CARGO_TARGET_DIR = 'C:\t\graphshell-target'
   cargo test -p buckram -p livery -p genet-livery --offline
   cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings
   rustfmt --edition 2024 --check <touched Rust files>
   git diff --check
   cargo build -p genet-wpt --release --all-features --offline
   ```

If K4g5 changes `paint_list_api`, its provider repository runs its own unit,
serialization, translator, and renderer image tests before the Genet consumer
commit.

Store generated maps, winner ledgers, browser measurements, commands, and
images under:

`<workspace>\testing\genet\wpt-ledger\<date>_buckram_k4g<gate>`

Keep proof outputs out of Git.

## Receipt template

Append one receipt beneath the completed sub-gate:

```markdown
### K4gN receipt - YYYY-MM-DD

Base commit:

Capability:

Boundary retained:

Pure fixture:

Adapter fixture:

Sizing fixture:

Live fixture:

Interop decision:

WPT exact movement:

Image or command proof:

Deferral counts:

Removal:

Verification:

Proof directory:

Commit:
```

## First executable task

The initial handoff is:

> Read this plan, the accepted K4f receipt, CSS 2.1 sections 17.6.2 and
> 17.6.2.1, and the live seams named under K4g1. Execute K4g1 only. Preserve
> unrelated worktree changes. Record the accepted K4f commit and freeze fresh
> `css/CSS2/tables`, `css/css-tables`, and `css/css-writing-modes` maps before
> changing layout or paint behavior. Stop after K4g1 passes its verification
> ladder, append its receipt here, stage only K4g1 paths, and commit. Do not
> begin K4g2 in the same task.
