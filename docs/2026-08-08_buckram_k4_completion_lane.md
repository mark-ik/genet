# Buckram K4 completion lane

**Date:** 2026-08-08

**Status:** B0 accepted. B1 is the next executable gate. The lane closes K4
and stops before K5 implementation.

**Architectural authority:** [Buckram CSS layout engine
plan](2026-07-26_buckram_css_layout_engine_plan.md)

**Table authority:** [Buckram K4 CSS tables execution
plan](2026-07-28_buckram_k4_css_tables_execution_plan.md)

**Collapsed-border detail:** [Buckram K4g collapsed-border execution
plan](2026-07-28_buckram_k4g_collapsed_border_execution_plan.md)

**Color dependency:** [Livery contextual color computation
plan](2026-07-28_livery_contextual_color_computation_plan.md)

## Charge

Take the accepted K4g1 tree through one standards-owned unfragmented table
path, delete the remaining table-to-Grid and row-to-Flex compatibility path,
append the K4 closure receipt, and stop.

This is a serial implementation lane for one agent. It includes the Livery
color gates that directly block collapsed-border integration. It does not
include general positioning, persistent relayout, fragmentation, Pelt engine
selection, HTML presentational hints, or Stylo retirement.

The lane exists because the parent plans contain three kinds of unfinished
work that were previously interleaved:

1. K4g1 supplied collapsed-border topology but not conflict resolution,
   metrics, sizing integration, paint, or mutation handling.
2. K4d's planned bridge-deletion receipt did not land. Live
   `place_table_cell`, `table_is_flattenable`, table-to-Grid, and row-to-Flex
   paths remain because later K4 deferrals can still select them.
3. K4d through K4f receipts name table-model and separated-paint work that has
   no executable handoff: row-group height, inline-table baseline use,
   vertical-flow captions, collapsed-track cell clipping, layered table
   backgrounds, DOM-order cell paint, `empty-cells`, and border-spacing paint.

K4 is not closed by running K4g2 through K4g6 alone. This lane schedules those
stranded items before the final bridge deletion.

## Accepted base

- K0 through K3 are accepted.
- K4a through K4e have accepted capability receipts, with the residuals named
  below still open.
- K4f's collapsed-track capability is accepted at `bad53b5cb2f`; the rest of
  K4f's separated-paint outcome is open.
- K4g1 is accepted at `19b91b6ebef`.
- Contextual-color C0 is accepted; C1 is the first gate in this lane.
- K5, K6, and K7 have not begun implementation.

At B0 entry, record the current branch and `git rev-parse HEAD`. Do not reset
the worktree to an accepted commit. Existing unrelated changes belong to the
current checkout and must be preserved.

## Ownership

Buckram owns:

- table topology, sizing, row layout, fragments, border conflict resolution,
  border metrics, and logical border geometry;
- one standards-shaped table result under both separated and collapsed border
  models; and
- explicit deferrals whose later owner is named.

Livery and `genet-livery` own:

- computed color representation and used-color resolution;
- translation from computed physical sides into Buckram logical inputs;
- retained-document invalidation; and
- lowering accepted Buckram fragments and paint geometry to neutral paint
  commands.

The neutral paint API owns reusable path, stroke, and border primitives. It
does not own CSS table conflict rules.

Taffy may still format flex or grid content inside a table cell. It may not
represent the table, a table row, or table track selection after B10.

## Lane order

| Gate | Outcome |
|---|---|
| B0 | Livery C1 supplies one authoritative computed color value |
| B1 | K4g2 selects one collapsed-border winner per atomic segment |
| B2 | K4g3 selects spanning-side behavior and projects layout metrics |
| B3 | stranded table sizing and wrapper geometry are closed |
| B4 | K4g4 reruns K4c/K4d with collapsed metrics |
| B5 | the unfinished K4f separated table paint model is completed |
| B6 | Livery C2 supplies explicit scheme and system-color context |
| B7 | Livery C3 moves every color consumer to used-value resolution |
| B8 | K4g5 emits final collapsed-border geometry and paint |
| B9 | K4g6 handles mutation and removes collapsed-border fallbacks |
| B10 | K4h deletes the table bridge, audits deferrals, and closes K4 |

Accepted implementation gates land in this order. A gate gets its own receipt
and commit. Research artifacts may be prepared ahead, but later behavior does
not land early.

## B0. One authoritative computed color

Execute C1 exactly as specified by the contextual-color plan.

Primary seams:

- `components/livery/src/values/color/`;
- `components/livery/src/values/property.rs`;
- `components/livery/src/cascade.rs`; and
- `components/livery/build.rs`.

The accepted result preserves contextual expressions through declaration
parsing, `var()`, CSS-wide keywords, inheritance, generated field copies,
gradients, shadows, and decoration values. It does not yet choose a system
palette or lower colors to paint.

**Receipt:** entered `main` at
`4c3c304206900e42ea877936a7f901e1f447ed96`. C1 now stores one non-`Copy`
`ComputedColor` through generated color longhands, gradients, shadows, and
text-decoration aliases; parser and computed-value construction perform no
palette lookup. Focused C1 and full Livery receipts pass, as do all
`genet-livery` targets. The committed contextual-color plan carries exact
counts and the remaining C2/C3 boundary.

**Stop after B0.** Run the C1 receipt, stage only the color-model paths, and
commit. Do not combine the color representation change with table adapter
work.

## B1. Atomic conflict winner

Execute K4g2 from the collapsed-border plan.

Primary seams:

- `components/buckram/src/table/borders.rs`;
- `components/buckram/src/table.rs` and `components/buckram/src/lib.rs` for
  public table contracts;
- `components/genet-livery/src/table_sizing.rs`;
- `components/genet-livery/src/table_block.rs`; and
- the computed-side adapter in `components/genet-livery/src/style.rs` or one
  table-specific adapter module proved by the implementation.

Map physical computed sides into the table's logical axes once. Fill the
direction-corrected `TableBorderOrderKey`, then resolve CSS2 precedence with
one pure comparator. Preserve the winning computed color expression as data;
color does not participate in the ranking.

**Receipt:** pairwise, permutation, LTR/RTL, anonymous-source, `hidden`, and
all-`none` fixtures prove a total deterministic comparator. A source audit
finds one winner selector.

## B2. Spanning-side rule and collapsed metrics

Execute K4g3. Use the recorded Chrome/Firefox matrix as prior research, then
recheck the exact cases that select the accepted rule. Do not repeat a broad
browser survey unless a focused recheck disagrees with the recorded evidence.

Keep atomic winners even if the accepted interop rule harmonizes a connected
side. Project explicit inline, block, outer-edge, and overflow metrics from
those winners. CSS-pixel half widths remain unsnapped.

**Receipt:** every metric traces back to exact atomic winners. The receipt
records the selected spanning-side rule and any stable split retained as a
named interoperability deferral.

## B3. Stranded table geometry

Close the model work named by accepted K4d and K4e receipts before rerunning
the table algorithms under collapsed metrics.

Primary seams:

- `components/buckram/src/table/rows.rs`;
- `components/buckram/src/table/pipeline.rs`;
- `components/buckram/src/table/fragments.rs`;
- `components/genet-livery/src/table_block.rs`;
- `components/genet-livery/src/table_wrapper.rs`;
- `components/genet-livery/src/layout.rs`; and
- `components/genet-livery/src/text.rs`.

Work:

1. Carry row-group block-size constraints into `TableBlockSizingInput` and
   apply a definite row-group height over that group's rows using the accepted
   K4d interop rule.
2. Make an inline-table atom consume K4d5's exported first table baseline
   rather than the bottom of its margin box.
3. Place captions through the table's logical block axis. Vertical writing
   mode is table-wrapper work, not fragmentation work. The K4e3 receipt routed
   vertical-flow captions to K6; this lane corrects that routing.
4. Refresh the 2026-08-03 deferral census and classify every surviving
   counter from live counts, including `CaptionMinPendingK4e`,
   `PercentagePaddingPendingBasis`, and whatever remains of
   `GridSizeMismatch`, `InvalidConstraint`, and `FixedLayoutWithoutColumns`
   after the empty-table fix. A reachable case must gain a standards-owned
   input or an exact later owner. It may not select the table bridge merely
   because a basis is indefinite.
5. Preserve `getClientRects()` as a general multi-fragment API gap. Do not
   invent that API inside K4.

Split B3 into separate commits if row sizing and inline wrapper integration
touch independent acceptance surfaces. The lane order remains B3 before B4.

**Receipt:** row-group, vertical-flow caption, inline-table baseline, and
deferral-census fixtures pass on the Buckram table route. The receipt names
every surviving non-K4 owner.

## B4. Collapsed sizing and overflow

Execute K4g4. Feed B2 metrics into the accepted K4c and K4d algorithms. Do
not fork fixed sizing, automatic sizing, or row layout for collapsed mode.

Primary seams:

- `components/buckram/src/table/{fixed,automatic,automatic_used,rows,sizing,pipeline,fragments}.rs`;
- `components/genet-livery/src/table_sizing.rs`;
- `components/genet-livery/src/table_block.rs`; and
- `components/genet-livery/src/table_shadow.rs`.

**Receipt:** sums reconcile content, padding, interior winners, half outer
winners, table size, and overflow. K4c/K4d fixtures rerun under both border
models. Accepted collapsed cases no longer produce either K4g metric
deferral.

## B5. Separated table paint closure

Complete the unfinished portion of K4f before K4g5 relies on the table paint
phase.

Primary seams:

- `components/buckram/src/table/fragments.rs` for paint-relevant table
  structure only;
- `components/genet-livery/src/paint.rs`;
- `components/genet-livery/src/layout.rs`; and
- `components/genet-livery/tests/paint.rs` plus focused interaction/image
  fixtures.

Work:

1. Paint table, column-group, column, row-group, row, and cell background
   layers from their own fragments in table paint order.
2. Paint cells in DOM order even when spans change grid position.
3. Implement `empty-cells: show | hide` from actual cell content state.
4. Paint border-spacing gaps without adding spacing to geometry a second
   time.
5. Clip a cell spanning a collapsed track at the accepted track edge, then
   remove the adapter rule that keeps every spanned track visible.
6. Account for table background clipping, box shadow, and overflow at the
   wrapper/grid boundary.

B5 lands before the B6/B7 color gates. Its command and image fixtures use
plain authored colors so the later color migration cannot invalidate them;
scheme-dependent and system colors are not B5 evidence.

**Receipt:** command and image fixtures distinguish every background layer,
DOM order, empty cells, spacing, and collapsed-track clipping. Generic block
paint assumptions no longer decide table-internal paint order.

## B6. Computed color context

Execute contextual-color C2 as a separate gate. Add element `color-scheme`,
host preference, used scheme, and a host-owned system palette. Resolve system
colors at computed-value time under the correct element scheme.

**Stop after B6.** Scheme/palette computation and downstream paint migration
remain separately attributable.

## B7. Color observables and consumers

Execute contextual-color C3. CSSOM, backgrounds, borders, decoration,
gradients, shadows, text, and animation must resolve the authoritative
computed color through an explicit used-value context. Remove the black
fallback for a valid unresolved expression.

**Receipt:** CSSOM and headed paint agree after inheritance, scheme changes,
palette changes, and one animation sample. K4g may now accept headed color
evidence.

## B8. Collapsed-border geometry and paint

Execute K4g5. Derive segment endpoints from B4's final grid lines, retain
logical geometry until the final flow transform, and emit each winner once in
the table phase established by B5.

Primary seams:

- `components/buckram/src/table/borders.rs` plus a dedicated geometry module
  if the model warrants one;
- `components/genet-livery/src/paint.rs`;
- neutral paint command types only if existing path, stroke, and border
  primitives cannot express the accepted joins; and
- command/image fixtures at device scales 1 and 2.

A neutral paint API change is its own provider commit with renderer receipts.
The Genet consumer commit follows it.

**Receipt:** suppressed segments are absent, each winner is emitted once,
every accepted style is represented, and generic per-cell border commands are
suppressed in collapsed mode.

## B9. Dynamic collapsed-border closure

Execute K4g6. Recompute candidates, winners, metrics, geometry, and paint when
participating styles, roles, spans, track visibility, structure, direction,
or writing mode change. A color-only change preserves geometry; a winning
width change reruns K4c and K4d.

Delete collapsed-as-separated sizing and paint paths, duplicate winner logic,
and accepted K4g deferral variants. Remove the `table-layout` partial marker
only after the collapsed K4c/K4d receipt passes.

**Receipt:** source and command audits find one candidate grid, one winner
selector, one metrics path, and one paint lowering. Mutation fixtures cannot
leave layout and paint on different winner generations.

## B10. K4h bridge deletion and closure

K4d planned to delete the compatibility bridge but its accepted live slice
retained it for later-K4 deferrals. B10 performs the deletion; it does not
merely audit an earlier removal.

Primary deletion seams:

- `components/genet-livery/src/layout.rs`:
  `place_table_cell`, `table_is_flattenable`, table bridge counters, and
  table/row backend style mutations;
- any table-to-Grid or row-to-Flex selection in `genet-livery` or Buckram's
  Taffy adapter; and
- obsolete table deferral variants, compatibility switches, and shadow
  comparisons whose acceptance purpose has ended.

Work:

1. Apply relative offsets to preserved table-part fragments and expose the
   correct wrapper/internal containing fragments for K5.
2. Inventory every table deferral and compatibility flag from source and live
   counters.
3. Route general absolute, fixed, and sticky behavior to K5 and
   fragmentation-dependent behavior to K6 while keeping each table on the
   Buckram dispatcher.
4. Route genuinely foundational sizing cycles to K7 without reviving table
   Grid/Flex dispatch.
5. Delete the compatibility bridge and prove flex/grid content inside cells
   remains the only table-adjacent Taffy use.
6. Append the exact K4 receipt to the parent and master plans.

**Stop after B10.** Do not begin K5 implementation in the same task.

## Verification ladder

Every behavior-changing gate runs the smallest owning tests first, then the
shared table surface:

```powershell
cargo test -p buckram --lib --offline
cargo test -p livery --offline
cargo test -p genet-livery --all-targets --offline
cargo clippy -p buckram -p livery -p genet-livery --no-deps --offline -- -D warnings
cargo build -p genet-wpt --release --all-features --offline
rustfmt --edition 2024 --check <touched Rust files>
git diff --check
```

If the combined strict Clippy command fails only on an untouched known
warning, prove that source byte-identical to the accepted base, run strict
Clippy on every touched package, and report the combined command as blocked.
Do not turn a partial strict run into a pass claim.

Use the gate-specific WPT families named in the K4g plan. At B3, B4, B5, B8,
B9, and B10, refresh complete `css/CSS2/tables`, `css/css-tables`, and
`css/css-writing-modes` maps; B3 belongs in that list because row-group
heights, captions, and inline-table baselines move live geometry. Run the all-nine comparison whenever shared
sizing, fragments, paint, or writing-mode behavior moves.

Generated maps, screenshots, and proof builds stay under a gate-specific
`testing/genet/wpt-ledger` directory outside Git. Each receipt distinguishes:

- pure Buckram model proof;
- Livery adapter proof;
- live unheaded behavior;
- headed paint behavior; and
- incumbent differential movement.

Only the first four can establish K4 capability. Incumbent movement is a
regression signal.

## Working-tree and commit discipline

- Preserve unrelated worktree changes.
- Stage only the current gate's files.
- Record the accepted base and resulting commit in the gate receipt.
- Keep implementation, generated proof, and expectation-map changes
  attributable.
- Stop when a named gate passes. Begin the next gate in a fresh task or
  explicit continuation.

## Lane stop rules

- Stop if HTML attributes enter Buckram.
- Stop if conflict resolution happens during sizing or paint.
- Stop if collapsed mode forks K4c or K4d into a second sizing algorithm.
- Stop if K4f paint reconstructs group or column geometry from cells.
- Stop if a table deferral selects the old Grid/Flex table engine after B10.
- Stop if `genet-layout` becomes a source of CSS semantics.
- Stop if a K5, K6, K7, presentational-hint, Pelt-route, or Stylo-retirement
  concern is implemented without its owning gate.
- Stop if a WPT pass is credited to Buckram while the compatibility bridge
  supplied the result.

## Done condition

The lane is complete when:

1. separated and collapsed tables share one Buckram topology, sizing, row,
   fragment, and wrapper path;
2. collapsed borders have one candidate grid, one accepted winner rule, one
   metric projection, and one paint lowering;
3. K4d/K4e/K4f residuals named in this lane have receipts or an exact K5-K7
   owner that does not require table fallback;
4. live mutation keeps sizing and paint on the same border generation;
5. `place_table_cell`, `table_is_flattenable`, table-to-Grid, row-to-Flex,
   and table-specific Taffy style mutations are deleted;
6. every unfragmented table stays on Buckram even when a later positioning,
   fragmentation, or foundational sizing feature is unsupported; and
7. the master plan contains the accepted K4 receipt and names K5a as the next
   architecture gate.
