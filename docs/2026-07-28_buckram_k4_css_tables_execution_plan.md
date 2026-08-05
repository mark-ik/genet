# Buckram K4 CSS tables execution plan

**Date:** 2026-07-28

**Status:** in execution. K4a through K4c are accepted: Buckram owns live
table inline sizing on every route (K4c5b, `a96fe7d147e`), with the named
K4e, K4f, and K4g inputs still pending by design. K4d is in execution; K4d1
is accepted and K4d2 is next.

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

### What the standard says - 2026-08-04

Read after the 2026-08-03 revert, because the property split is specified
exactly and the revert was rediscovering it the expensive way.

**CSS 2.1 section 17.4** gives the list verbatim:

> The computed values of properties 'position', 'float', 'margin-\*', 'top',
> 'right', 'bottom', and 'left' on the table element are used on the table
> wrapper box and not the table box.

and the complement:

> all other values of non-inheritable properties are used on the table box
> and not the table wrapper box.

So `width`, `height`, `border`, `padding`, and `background` stay on the grid.
That is the whole of K4e1's migration, and both revert mechanisms are on the
list: `position` for the four `absolute-tables-*` cases, and the wrapper
being the flow participant for the two flex-item cases.

**CSS Tables 3 section 3.6.1 extends the list** with the properties that
establish a containing block or a stacking context: `overflow`, `opacity`,
`filter`, `clip`, `clip-path`, `isolation`, `mask-*`, `mix-blend-mode`,
`transform-*`, and `perspective`. It also names the mechanism, which is not
"zero them on the grid":

> Where these values aren't applied to the table grid/wrapper, unset values
> are used instead.

The grid takes the *initial or inherited* value, so the migration is a
per-property reset rather than a hand-written zero.

**The wrapper's display is specified, and it replaces the shrink-to-fit
hack.** CSS Tables 3: the wrapper is `inline-block` for an `inline-table`
and `block` for a `table`, and it establishes a block formatting context.
The `float: left` in `to_taffy_style` is standing in for exactly that
`inline-block`, which is why K4e2 can delete it rather than reproduce it.

**Percentages are already specified for the abspos case.** CSS 2.1:

> Percentages on 'width' and 'height' on the table are relative to the table
> wrapper box's containing block, not the table wrapper box itself.

`absolute-tables-012` is `position: absolute; width: 50%`. With `position`
on the wrapper the wrapper is the positioned box, and the grid's `50%`
resolves against the wrapper's containing block rather than against the
wrapper. That is a rule to implement, not a behavior to discover.

**One more, for K4d5's exported baselines.** CSS Tables 3: the table-root
box, not the wrapper, is used for baseline alignment of an inline-table.

#### The wrapper is sized from the grid, not from its containing block

CSS Tables 3 section 3.1: a table `width` that computes to `auto` behaves as
if `fit-content` were specified, so the grid shrink-wraps its content.
Section 2.2.1 then makes the wrapper's width the grid's border-edge width.

**So a `block` wrapper is not an ordinary block.** An ordinary block with
`width: auto` fills its containing block; this one takes its width from its
child. That is what the `float: left` in `to_taffy_style` has been
approximating, and it means K4e2 cannot simply drop the hack and mark the
wrapper `Display::Block` - it has to size the wrapper from the grid, which
in this tree is intrinsic sizing rather than block auto-width.

#### Two things the standard does not settle

Both have conventional answers and neither blocks K4e1, but they are
assumptions rather than citations and should be labeled as such wherever
they land:

- **The grid's containing block inside the wrapper.** Neither spec defines it
  explicitly. The percentage rule above routes around the question for the
  table's own `width` and `height`, which are the cases that would otherwise
  be circular.
- **What the anonymous wrapper inherits.** CSS 2.1's general rule gives an
  anonymous box its parent's inherited values, which here is the table
  element's *parent*, while the migrated properties come from the table
  element's own computed values. That combination is consistent but is not
  stated anywhere.

#### It also explains the caption divergence

CSS Tables 3 section 2.2.1 says the wrapper's width *is* the border-edge
width of the grid inside it. A caption wider than the grid cannot satisfy
that rule and be laid out at its min-content width at the same time, and the
two engines break the tie in opposite directions. Measured with
`which-box.html` in the K4e1 proof directory, where the **row** discriminates
the grid from the wrapper because a row spans the grid while a cell spans one
column:

| | Chrome 150 | Firefox 153 |
|---|---|---|
| table element rect | 176 | 176 |
| row | **176** | **8.8** |
| row, table with a 2px border | 172 | 8.8 |

Chrome keeps the spec's rule and grows the **grid** to the caption. Firefox
keeps the grid at its content width and grows the **wrapper**, breaking that
rule. Both give the table element the same outer box, so the difference is
invisible to `getBoundingClientRect` on the table and shows up only inside.

That confirms the caption matrix's reading rather than overturning it, and
it upgrades the K4e3 decision from picking an engine to picking which rule
to keep.

### Sub-gates

K4e lands serially like the rest. The order is forced by what each step
needs from the one before, and the caption question above is the clearest
case: it cannot be answered until the wrapper exists as a box.

- **K4e1. Wrapper participation, as one step.** The wrapper becomes a real
  node with the grid as its single child, *and* margins, float, position,
  and outer display move from the grid to it in the same change. The grid
  keeps width, borders, padding, and table-ness. Both emit fragments.

  **This cannot be split into "add the box" and "move the properties",
  which was tried and reverted on 2026-08-03.** Introducing the wrapper
  alone moved seven reftests in `css/css-tables` with no improvements,
  58 to 51, and the two mechanisms are both structural rather than
  incidental:

  - `table-as-item-cell-percentage-002` and `-003` put the table in a flex
    container. Inserting a wrapper makes the *wrapper* the flex item and
    the table an ordinary block inside it.
  - `absolute-tables-008`, `-010`, `-011`, and `-012` put
    `position: absolute` on the table element. That property belongs to the
    wrapper under CSS 2.1 section 17.4, so while it still sits on the grid
    the abspos box ends up nested inside a static wrapper that should not
    exist in its containing-block chain.

  Both are the property split, arriving early. The intermediate state is
  not a smaller step but a wrong one: the box being added is exactly the box
  that has to carry the properties.
### K4e1 receipt - 2026-08-04

Base commit: `5e69b1ca895`.

**Capability:** the table wrapper box is a node in the layout tree with its
own fragment, and one table element's computed values are split across it and
the grid by the lists CSS 2.1 section 17.4 and CSS Tables 3 section 3.6.1
give. `components/genet-livery/src/table_wrapper.rs` owns the split; the
wrapper is built in both `build_box` routes instead of being bypassed.

**The split is data, not code.** Livery's generated `ComputedValues` already
had `copy_property_from(PropertyId, &Self)` and `for_child`, so the migration
is a list of `PropertyId`s copied one way for the wrapper and copied from a
default style the other way for the grid. That is section 3.6.1's own
mechanism - "where these values aren't applied to the table grid/wrapper,
unset values are used instead" - rather than a hand-written zero, and adding a
property to the list is a one-line change.

**Narrowed to what Livery computes.** Section 3.6.1's `clip`, `clip-path`,
`filter`, `isolation`, `mask-*`, `mix-blend-mode`, `perspective`, and the
`transform-*` longhands past `transform` are all `[[unimplemented]]` in
`properties.toml`; they have no computed value to move and join the list the
day Livery grows them. `opacity` and `transform` are on the list and migrate,
but are not yet observable, because paint reads both from the style plane by
DOM node rather than from either box. Naming the box is K4e4's.

**Three rules had to come with the box, and all three came from the
specification rather than from tuning.** Each was found by laying the case out
and printing the geometry, not by reading the code:

1. **A percentage table size skips the wrapper.** The grid's `width: 50%`
   resolved against a wrapper that was itself waiting on the grid, and the
   pair measured 0 wide. CSS 2.1 section 17.4 already says percentages on a
   table's width and height are relative to the *wrapper's containing block*,
   so `grid_style` resolves them against that basis up front.
   (`absolute-tables-012`)
2. **The wrapper's width is the grid's border-edge width.** As a flex item the
   wrapper stretched to its container, 320 against a grid of 100. CSS Tables 3
   section 2.2.1 makes its width the grid's, so it is never `auto` in the sense
   stretching acts on. `wrapper_width_from_grid` applies that whenever the
   table's width is definite; the `auto` case is K4e2.
   (`table-as-item-cell-percentage-002`, `-003`)
3. **The shrink-to-fit hack is block-level only.** Applied to every wrapper it
   floated the `inline-block` wrapper of an `inline-table`, taking four
   side-by-side tables off the line. An inline-table wrapper is an atomic
   inline, an absolutely positioned wrapper is shrink-to-fit under CSS 2.1
   section 10.3.7, and a flex or grid item is sized by its container - all
   three already shrink-wrap, and floating them removes them from the
   formatting context they belong to.
   (`border-collapse-empty-row`)

**Correction to this gate's own scoping.** The text above predicted the six
named tests would *improve*. Three of them are not in the corpus at all
(`absolute-tables-008`, `-010`, `-011`); the other three were already passing,
and the 2026-08-03 attempt broke them. K4e1 keeps them passing. The prediction
was wrong about the direction and right about the mechanism.

**Boundary retained:** the bridge is untouched. `grid_style` runs only on a
box whose role is `Grid` and whose origin is an element's principal box, so a
wrapper generated by fixup around stray table parts stays an ordinary
anonymous block and migrates nothing.

**Pure fixture:** six tests in `table_wrapper.rs` covering both directions of
the split, `unset` rather than zero, the properties that stay, the percentage
basis, and the inline-table wrapper's display.

**Adapter fixture:** three tests in `layout.rs` -
`k4e1_the_wrapper_takes_the_margin_and_the_grid_keeps_its_own_box`,
`k4e1_a_percentage_table_skips_the_wrapper_it_would_otherwise_wait_on`, and
`k4e1_a_table_flex_item_is_the_wrapper_and_keeps_the_grids_width` - each
asserting through emitted fragment geometry.

**WPT:** both corpora rerun with **zero movement**, `unexpected=0`, at 58 and
65. Proof directory `testing/genet/wpt-ledger/2026-08-04_buckram_k4e1`.

**Verification:** buckram 163, 507 tests across the three crates, 0 failed.
`cargo clippy -p buckram` clean. The combined Clippy command is blocked by
four pre-existing warnings under Clippy 1.97.0 in code this gate does not
touch, verified byte-identical at `5e69b1ca895`: `too_many_arguments` on
`collect_fragments` and `collect_inline_fragments` in `layout.rs` and on
`table_block_inputs` in `table_block.rs`, and `implicit_saturating_sub` in
`text.rs`. Rustfmt and `git diff --check` clean on touched files.

**Not yet done in K4e1:** the wrapper does not shrink-to-fit an `auto`-width
table on its own - it still borrows `float: left`, now relocated from the grid
to the wrapper where the box that participates in flow actually is. Captions
are children of the wrapper but are not laid out above or below the grid. The
wrapper skip in `build_box` is gone, but the wrapper/grid exclusion in
`box_is_inline` remains.

- **K4e2. Block table and inline-table sizing.** Shrink-to-fit through the
  wrapper rather than through the `float: left` compatibility hack, auto
  margins, and float avoidance on K3's block equations. The hack now sits on
  the wrapper, and `wrapper_width_from_grid` is the definite-width half of the
  rule that replaces it; K4e2 owns the `auto` half, which is intrinsic sizing
  through the wrapper rather than block auto-width.
### K4e2 receipt - 2026-08-04

Base commit: `8d24c0d14d4`.

**Capability:** the wrapper takes the grid's border-edge width and
participates in normal flow. `float: left` no longer stands in for
shrink-to-fit on any table Buckram sizes.

**The rule turned out to be an assignment, not a measurement.** CSS Tables 3
section 2.2.1 makes the wrapper's width the grid's border-edge width, and
Buckram's table inline sizing already computes that number before the main
layout pass. The shrink-wrapping happens inside the table algorithm that owns
it; the wrapper reads the answer. `size_wrapper_from_grid` assigns it as soon
as `buckram_table_columns` returns, and an `auto` table width is no harder
than a specified one.

That is what `used_grid_inline_size` is for, and the distinction cost one
failing test to find: `used_table_inline_size` is the used value of the
`width` property, which under `content-box` is the content box. A table with
`width: 100px; border: 5px; padding: 3px` gives 100 and 116; the wrapper wants
116.

**The route not taken.** The plan expected this to go through Buckram's
intrinsic shrink-to-fit lane, and it was built that way first: a third
shrink-to-fit root beside floats and atomic inlines, the wrapper marked as a
BFC, empty leaves admitted to intrinsic measurement, table children admitted
to the block lane. Each step uncovered the next, and the last one - a `Table`
child is an independent formatting context a Buckram block parent defers -
would have meant widening K3's block dispatch and giving `AlgorithmKind::Table`
an intrinsic size, which is a gate of its own. All of it was reverted once the
assignment turned out to be available. Buckram is unchanged by K4e2.

**Auto margins came free.** A float resolves `auto` margins to zero, so
`table { margin: 0 auto }` could not centre a table. An in-flow block with a
definite width centres the way any other block does, on K3's equations, with
no table-specific code.

**Float avoidance likewise.** The wrapper is an ordinary in-flow BFC now, and
moves below a float on the same route every other BFC uses.

**WPT:** `css/css-tables` 58 to **62**, `css/CSS2/tables` 65 to **118**. Net
**+57**, 58 improvements against 1 regression. Thirty of the improvements are
`fixed-table-layout-*`, where the table's width was right all along and its
place on the page was not - the float had it out of normal flow.
Proof directory `testing/genet/wpt-ledger/2026-08-04_buckram_k4e2`.

**The one regression is a false pass being exposed, and it is routed.**
`border-collapse-offset-001.xht` puts a collapsed-border table in an
absolutely positioned div; its reference renders the same markup with
`cellspacing="0"`, so the reference's table is **separated** and Buckram sizes
it. Measured directly: the abspos div is 8 wide in both documents before
K4e2 - its borders and nothing else - and 118 in the reference after, which is
correct. The test still measures 8 because a deferred table has no width to
assign and its wrapper keeps the fallback float, which an abspos box's
shrink-to-fit does not include. Both documents were wrong the same way and so
matched; one is now right. Collapsed borders are 201 of the deferral set, and
this closes with K4g and needs nothing here.

Same two-document trap as `table-cell-overflow-explicit-height-002` in K4d: a
reftest renders two files, and a skip that fires on one is invisible until the
pair disagrees.

**Pure fixture:** none. K4e2 adds no Buckram behavior.

**Adapter fixture:** `k4e2_an_auto_width_wrapper_measures_the_grid_instead_of_filling`
and `k4e2_auto_margins_centre_a_table`, both asserting through emitted
fragment geometry. The first is the case the float hack could reach and the
assignment now reaches exactly; the second is the case the float hack could
not reach at all.

**Verification:** 509 tests across the three crates, 0 failed.
`cargo clippy -p buckram` clean; `-p genet-livery` blocked by the same four
pre-existing Clippy 1.97.0 warnings recorded in the K4e1 receipt. Rustfmt and
`git diff --check` clean on touched files.

**Not yet done in K4e2:** the `float: left` fallback survives for tables
Buckram defers, and its domain is now exactly that set. It is applied when the
tree is built and retired the moment a width arrives - and only where this
route put it there, never where the author wrote `float` on the table and K4e1
migrated it onto the wrapper. `wrapper_width_from_grid` also stays, because a
flex or grid item's style has to be right before layout rather than after.

- **K4e3. Captions.** Resolve the divergence measured above, lay captions
  out between the wrapper's margins and the table's borders, and decide
  whether `CaptionMinContribution` belongs to wrapper sizing or grid sizing.
- **K4e4. CSSOM, hit testing, and removal.** Make wrapper/grid/caption
  selection explicit, and delete the remaining compatibility routes: the
  wrapper/grid exclusion in `box_is_inline`, and `wrapper_needs_float_fallback`
  once K4g has emptied the deferral set that is now its whole domain. The
  wrapper skip in `build_box` and the table `float: left` in `to_taffy_style`
  are already gone as of K4e1. Paint and
  `getBoundingClientRect` currently read the style plane by DOM node and take
  whichever of the two boxes comes first, which is the wrapper; that is the
  accident this sub-gate replaces with a decision, and it is what makes the
  migrated `opacity` and `transform` observable.

### K4e caption interop matrix - 2026-08-03 (research, ahead of its gate)

What a caption contributes to table inline sizing, measured before
implementing the `CaptionMinContribution::PendingK4e` seam that the
2026-08-03 deferral census counted firing 17 times. Headless **Chrome
150.0.0.0** (`--dump-dom`) and **Firefox 153.0** (`--screenshot`), matrix in
the K4e1 proof directory. Chrome's subpixel figures are rounded here.

| Case | Chrome 150 | Firefox 153 |
|---|---|---|
| C1 caption min-content wider, table auto | table 176, **cell 176** | table 176, **cell 8.8** |
| C2 caption narrower than the table | table 176, cell 176 | same |
| C3 caption wider, table `width: 50px` | table 176, **cell 176** | table 176, **cell 50** |
| C4 `caption-side: bottom`, caption wider | table 176, **cell 176** | table 176, **cell 8.8** |
| C5 caption `margin-left: 30px`, wider | table 206, cell 206 | table 206, cell 8.8 |
| C6 breakable caption text | table 88 | same |
| C7 caption `width: 300px` | table 300, cell 300 | table 300, cell 8.8 |

**Both engines agree on every table width.** The caption's contribution is
its **min-content** width (C6: breakable text contributes one run, 88, not
the full 264), it **includes the caption's own margins** (C5: 176 + 30), a
specified caption width participates normally (C7), and `caption-side` does
not affect it (C4).

**They diverge on whether the grid stretches to it.** Chrome widens the
table grid and its columns to the caption-imposed width; Firefox leaves the
grid at its own content width and lets only the wrapper be caption-wide. C3
is the sharpest: with `width: 50px` on the table, Firefox keeps a 50px grid
while Chrome discards the authored width and produces 176.

**This changes what the existing seam should do, so it is deliberately not
implemented yet.** `caption_min` currently feeds
`used_table_inline_size` in `size_automatic_table_inline`, where it is maxed
against the constrained table size. That encodes **Chrome's** behavior,
including C3's override of an authored `width`, and it was wired in before
this was measured. Under CSS 2.1 section 17.4 the captions' containing block
is the wrapper, and the wrapper is exactly the box K4e introduces, so a
caption plausibly belongs to wrapper sizing rather than to grid sizing at
all.

Deciding that is K4e's work and it needs the wrapper to exist first.
Measuring the caption now and feeding the existing seam would have silently
committed to Chrome's reading of C3 without anyone choosing it, which is why
the 17 deferrals stay for the moment. They are a named gap, not a defect.

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

## Current executable task

K4a and K4b are accepted below. The current handoff is K4c1 from the
[K4c execution plan](2026-07-28_buckram_k4c_table_inline_sizing_execution_plan.md):

> Read the K4c plan, the accepted K4b receipt, CSS 2.1 sections 17.5.2.1 and
> 17.5.2.2, and the live seams named under K4c1. Execute K4c1 only. Record
> `26eda4cd9fe` as the accepted K4b base and produce fresh
> `css/CSS2/tables` and `css/css-tables` maps before changing layout behavior.
> Stop after K4c1 passes its verification ladder, append its receipt to the
> K4c plan, stage only K4c1 paths, and commit. Do not begin K4c2 in the same
> task.

## K4a execution receipt (2026-07-28)

K4a began from the accepted K3 closure `2f1ae56968c`; the fresh maps were made
from the unchanged code checkout carrying the K4 plan. Before the code change,
the Livery reftest maps were:

| Corpus | Pass | Fail | Skip | Error |
|---|---:|---:|---:|---:|
| `css/CSS2/tables` | 66 | 184 | 889 | 0 |
| `css/css-tables` | 50 | 80 | 198 | 0 |

### Model and adapter proof

- Buckram now distinguishes the anonymous wrapper from the principal grid and
  retains independent provenance for both. Ordered repair covers improper
  children, missing row and row-group children, missing table parents,
  collapsible whitespace, and out-of-flow descendants.
- Livery computes `inline-table`, header/footer groups, column groups, and
  columns, and owns CSS 2.1 values for `border-collapse`, `border-spacing`,
  `caption-side`, and `empty-cells`. The HTML UA sheet gives `thead`, `tbody`,
  `tfoot`, `colgroup`, and `col` their distinct table roles.
- Buckram's pure model tests, Livery value tests, and the DOM-to-Buckram role
  test cover these distinctions. `cargo test -p buckram --offline` passed 76
  tests; `cargo test -p livery --offline` passed 32 tests; the full
  `cargo test -p genet-livery --offline` suite passed after the final bridge
  adjustment.

### Bridge and regression proof

The legacy Grid/Flex compatibility bridge stays attached to the grid. Its
temporary wrapper/grid route keeps the pair in block flow, including an
`inline-table`, until K4e supplies table dispatch, caption flow, and float
avoidance. This preserves the renderer's prior behavior while the CSS box tree
retains the correct inline outer role; it is not a claim that K4a implements
inline-table sizing or placement.

`cargo build -p genet-wpt --release --all-features --offline` succeeded. The
post-change reftest maps recorded no regressions:

| Corpus | Pass | Fail | Skip | Error | Delta |
|---|---:|---:|---:|---:|---|
| `css/CSS2/tables` | 67 | 183 | 889 | 0 | `border-collapse-empty-row.html` passed |
| `css/css-tables` | 53 | 77 | 198 | 0 | caption-relative-positioning and two percentage-grandchildren quirks tests passed |

The combined strict Clippy admission remains blocked by 147 pre-existing
Livery diagnostics, including `color/space/mod.rs:234` (`wrong_self_convention`);
it emitted no diagnostic for a K4a-changed path. Generated maps and command
logs are retained under `testing/genet/wpt-ledger/2026-07-28_buckram_k4a` and
remain outside Git.

## K4b execution receipt (2026-07-28)

K4b adds Buckram's typed `TableGrid`: wrapper and grid identity, visual-order
row groups, source-order column groups, tracks, cells, captions, slot
occupancy, and deterministic input errors. Header rows are placed before body
rows and footer rows after them without rewriting source boxes. Explicit and
implicit columns are tracks only, never invented CSS cell boxes.

Livery normalizes HTML attributes before this boundary: `colspan`, positive
`rowspan`, `rowspan="0"`, `col span`, and `colgroup span`, with HTML bounds of
1000 for column spans and 65534 for positive row spans. CSS-display tables do
not consume DOM span attributes. The old `table_cells` collector is deleted;
the temporary `place_table_cell` bridge receives only `TableGrid` start slots.
It intentionally does not translate a span into a Taffy `GridPlacement`.
K4c owns span sizing and K4d replaces the bridge.

### Evidence

- `cargo test -p buckram --offline` passed 82 tests, including simple rows,
  column and row spans, `rowspan="0"`, malformed duplicate input,
  columns/column groups, and header/body/footer order.
- K4b adapter tests prove HTML normalization and that CSS-display tables keep
  default spans. The full `cargo test -p genet-livery --offline` suite passed.
- `cargo build -p genet-wpt --release --all-features --offline` succeeded.
  Exact-source maps are unchanged from K4a: `css/CSS2/tables` is 67 pass, 183
  fail, 889 skip, 0 error; `css/css-tables` is 53 pass, 77 fail, 198 skip, 0
  error. There are no K4a-to-K4b status changes.
- Focused reftests pass for `zero-rowspan-001.html` and `row-group-order.html`.
  `table_grid_size_col_colspan.html` remains an existing local failure, and
  `colspan-001.html` plus `column-track-merging.html` are non-reftest skips.
- Strict Buckram Clippy reports no K4b diagnostic. The admission remains
  blocked by pre-existing `taffy_adapter.rs:3910` test code
  (`manual_is_multiple_of`); the broader Livery block remains recorded above.

Generated maps and command logs are retained under
`testing/genet/wpt-ledger/2026-07-28_buckram_k4b` and remain outside Git.
