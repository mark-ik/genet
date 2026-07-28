# Buckram: a CSS layout engine over reusable algorithms

**Date:** 2026-07-26
**Status:** active design; implementation starts at K0.
**Decision:** Buckram owns CSS box generation, formatting contexts, intrinsic
sizing, and fragments. Taffy is an algorithm library for flex and grid, with
block layout retained only as a migration aid.
**Absorbs:** the
[Livery box-tree plan](./2026-07-26_livery_box_tree_and_formatting_contexts_plan.md).
Its completed B-1/B0 receipt remains valid evidence and becomes Buckram's K0
starting receipt.
**Parent:** the
[Livery fullweb cutover plan](./2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md).
Buckram does not lower that plan's F4 cutover bar.

## The ruling

Livery is Genet's CSS style engine. Buckram becomes Genet's CSS layout engine.
They are separate because computed values and layout results have different
standards-owned models.

Taffy's low-level API is shaped for embedding its flex, grid, and block
algorithms in another tree. Buckram will use that API. It will not use
`TaffyTree` as the browser's box tree or `taffy::Layout` as the browser's
layout result.

The fragment model decides this boundary. CSS requires one box to produce many
fragments:

- an inline box split across line boxes;
- a paragraph continued through columns;
- a block continued across pages;
- a box resumed in another fragmentainer.

The current Livery and `genet-layout` outputs both reduce a node or box to one
bare rectangle. A `HashMap<NodeId, Layout>` cannot represent the relationship
between those fragments, their containing fragments, or their continuation
state. Adding fields to Taffy's `Style` cannot fix an output-model mismatch.

## Target shape

```text
DOM + Livery computed values
              |
              v
      Buckram CssBoxTree
              |
              v
 formatting-context dispatcher
   |          |           |
   |          |           +--> table algorithm owned by Buckram
   |          +--------------> inline algorithm owned by Buckram + Parley
   +-------------------------> flex/grid adapter -> Taffy low-level algorithms
              |
              v
        FragmentTree
              |
              +--> paint
              +--> hit testing
              +--> accessibility geometry
              +--> CSSOM used values
```

`genet-livery` remains the document integration lane during the cutover. The
new `components/buckram` crate owns the engine model and algorithms. Livery
supplies computed values through a narrow adapter and consumes Buckram's
fragments.

## Owned models

### Box tree

`CssBoxTree` carries:

- stable `BoxId` identity independent of DOM and Taffy nodes;
- DOM, pseudo-element, anonymous, and generated provenance;
- separate outer and inner display roles;
- table-internal roles and anonymous fixup;
- formatting-context establishment;
- positioning category and containing-block relationship;
- logical sizes, edges, and axes;
- replaced-content and intrinsic-size providers.

The existing B-1/B0 public model is the seed. It is not the final output model.

### Fragment tree

The first Buckram contract adds `FragmentId` and a real tree:

```rust
pub struct Fragment {
    pub id: FragmentId,
    pub box_id: BoxId,
    pub parent: Option<FragmentId>,
    pub containing_fragment: Option<FragmentId>,
    pub fragmentation_context: FragmentationContextId,
    pub logical_rect: LogicalRect,
    pub continuation: Option<BreakToken>,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
}

pub struct FragmentTree {
    roots: Vec<FragmentId>,
    fragments: SlotMap<FragmentId, Fragment>,
    by_box: HashMap<BoxId, Vec<FragmentId>>,
}
```

The exact storage may change. These invariants may not:

1. one box maps to zero, one, or many fragments;
2. every fragment has tree position and a coordinate space;
3. a continuation records where layout resumes;
4. consumers address fragments directly and recover box or DOM provenance
   through explicit maps;
5. logical geometry is primary and physical geometry is derived at the
   consumer edge.

### Formatting contexts and sizing

Buckram owns:

- block and inline formatting contexts;
- float exclusions, clearance, and margin collapsing;
- line construction, bidi ordering, baselines, and inline fragmentation;
- table fixup, track sizing, captions, spans, and border conflict resolution;
- containing blocks and static, relative, absolute, fixed, and sticky
  positioning;
- intrinsic-size queries and their cache;
- fragmentation contexts and break tokens;
- dirty-subtree invalidation and incremental relayout.

Intrinsic sizes are queries:

```rust
intrinsic_size(box_id, axis, MinContent | MaxContent) -> CSSPixels
```

They are not sentinel values smuggled through `auto`, and not private answers
that only a backend algorithm can see.

## What Taffy keeps

Buckram should retain the parts of Taffy that are already strong:

- the tree-agnostic low-level traits;
- `compute_flexbox_layout` and `compute_grid_layout`;
- compact length storage where it can represent the required CSS value;
- proven flex and grid algorithms;
- safe common-path Rust.

The adapter presents a Buckram subtree through `LayoutPartialTree`, supplies a
Buckram-owned style view, invokes the selected algorithm, and receives
placements into scratch storage. Buckram then creates or updates fragments.
Taffy node ids and `Layout` values do not escape the adapter.

Taffy's block algorithm may remain behind the same adapter during K1 so the
tree migration can prove zero movement. K3 decides which block pieces remain
useful once Buckram owns BFC, IFC, floats, logical geometry, and intrinsic
queries.

## Fork policy

The fragment-tree disagreement does not require a Taffy fork. Buckram owns the
result model and may call unchanged Taffy algorithms for an unfragmented flex
or grid formatting context.

Three kinds of Taffy change have different answers:

1. Safe constructors, public intrinsic-query hooks, and read-only diagnostics
   are additive API gaps. Upstream them where useful. Buckram may own the
   corresponding CSS value until an upstream release lands.
2. Genet-specific fixes to an otherwise reusable flex or grid algorithm may
   live temporarily in the existing narrow fork, with a patch log and upstream
   draft.
3. Fragmenting flex or grid requires break and resume behavior inside the
   algorithm. First propose a backend-neutral upstream API. If upstream cannot
   carry it, keep a minimal algorithm fork or extract the needed algorithm into
   Buckram. Do not deform `FragmentTree` back into one-layout-per-node to avoid
   that decision.

`support/patches/taffy` therefore survives Stylo retirement while Buckram needs
its documented patches. `stylo_taffy` and Stylo-only patches do not.

## Build order

### K0. FragmentTree foundation

**Files:**

- new `components/buckram/Cargo.toml`
- new `components/buckram/src/{lib,box_tree,fragment_tree}.rs`
- `components/genet-livery/src/{box_tree,layout,lib}.rs`
- current fragment consumers in paint, hit testing, accessibility, and CSSOM

Move the existing Taffy-free B-1/B0 box model into Buckram. Add
`FragmentTree`, initially with one principal fragment per laid-out box, and
compatibility views for consumers that still require the old planes. Move
each consumer to direct fragment identity, then delete its compatibility
view.

**Receipt:** `cargo test -p genet-livery` plus Buckram structural tests; the
same nine-directory corpus used by B0 remains exactly 5,744 passes with zero
moved files. The B0 receipt is preserved, not rerun as a claim that
fragmentation exists.

**Removal receipt:** browser-facing fragment types are Buckram types; the
single-rect maps remain private compatibility code with named consumers.

#### K0 receipt - 2026-07-26

`components/buckram` now owns `CssBoxTree`, `FragmentTree`, `BoxId`,
`FragmentId`, logical fragment geometry, parent and containing-fragment
relationships, fragmentation-context identity, and one-to-many box and node
lookups. Its six structural tests cover split-inline provenance, anonymous
table ownership, `none` versus `contents`, pseudo and replaced origins, a box
with two fragments, and fragment parentage.

`genet-livery` is now an adapter at this boundary. Its generated-box pass
supplies Livery values to Buckram's box roles, and each current Taffy pass
builds a Buckram `LayoutResult` rather than returning a node-to-rectangle
plane. The public `Fragment` is Buckram's identity-bearing fragment.
Paint, hit testing, the retained document, and CSSOM read `LiveryLayout`;
the old public `FragmentPlane` name is gone.

The remaining rectangle maps are private and named:

- `LegacyFragmentPlane` carries preliminary-pass geometry into the second
  inline pass.
- `LiveryLayout::atomic_fragments` carries inline-block and replaced-element
  geometry required by that same two-pass workaround.

Both are K2 deletion targets. They do not supply browser-facing fragment
identity.

Verification:

- `cargo test -p buckram`: 6 passed.
- `cargo test -p genet-livery`: 112 passed.
- `cargo clippy -p buckram --all-targets -- -D warnings`: passed.
- `cargo clippy -p genet-livery --all-targets --no-deps -- -D warnings`:
  passed. Dependency linting remains blocked by 147 existing warnings in the
  separate `livery` value crate.
- live-DOM structure: a generated child box resolves to its Buckram `BoxId`,
  and its fragment records the expected parent and containing fragment.

Exact reftest status diff, frozen B0 baseline
`Code/testing/genet/wpt-ledger/2026-07-26_boxtree_b0_final` against
`Code/testing/genet/wpt-ledger/2026-07-26_buckram_k0`:

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

Each current JSON has the same test-file cardinality as its frozen baseline.
This closes K0 without claiming a fragmentation algorithm.

### K1. Low-level Taffy adapter

**Files:**

- new `components/buckram/src/taffy_adapter.rs`
- `components/genet-livery/src/layout.rs`

Replace `TaffyTree` with a caller-owned tree implementing the low-level Taffy
traits. Dispatch block, flex, and grid explicitly. Treat returned layouts as
scratch placements used to construct fragments.

**Receipt:** the K0 all-nine corpus has zero moved files; flex and grid unit
fixtures retain their exact geometry; no public Buckram or Livery API exposes
a Taffy type.

**Removal receipt:** `genet-livery` has no `TaffyTree`, and DOM-node-to-Taffy
maps are gone.

#### K1 receipt - 2026-07-26

`components/buckram/src/taffy_adapter.rs` now owns a dense algorithm-node
arena and implements Taffy's low-level block, flex, grid, cache, traversal,
and rounding traits. Each node carries its Buckram source identity directly;
Livery's two source side maps are deleted. The adapter dispatches algorithms
explicitly and returns backend-neutral scratch geometry for fragment
construction.

No public Buckram API exposes a Taffy type. `genet-livery` contains neither
`TaffyTree` nor a bare Taffy `NodeId`, and Buckram's selected Taffy feature
graph does not contain `taffy_tree`. Livery still lowers CSS into Taffy's
private `Style`; K3 owns that input-model deletion.

Verification:

- `cargo test -p buckram`: 8 passed, including exact flex and 2-by-2 grid
  placement fixtures.
- `cargo test -p genet-livery`: 112 passed.
- strict Clippy passed for Buckram and genet-livery.

The frozen K0 corpus and K1 corpus each contain 16,375 tests. Every recorded
test kept the same status:

| directory | K0 pass | K1 pass | moved statuses |
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

This closes K1: Buckram owns the algorithm tree and Taffy is an implementation
backend, with no observed layout movement.

### K2. Box generation and one inline pass

Implement outer and inner display roles, anonymous block and table fixup,
inline boxes split around blocks, `display: contents`, list-item structure,
and pseudo/generated origins.

Replace Livery's preliminary layout plus text-measure layout workaround with
one inline formatting context that owns whitespace processing, shaping, bidi,
line breaking, inline continuations, and baselines. Parley shapes and breaks
text; Buckram constructs line and inline fragments.

**Receipt:** the existing B1 fixtures, named CSS2 anonymous-box and whitespace
families, bidi and baseline fixtures, and at least one inline box producing
two fragments. Delete the B0 suppressed/comment compatibility boundaries when
their proper box-generation rule replaces them.

#### K2 receipt - 2026-07-27

K2 is structurally complete. It is not a conformance-ratchet win.

Buckram now generates its own box tree from `BoxTreeInput`. The generator owns
outer and inner display roles, `none` versus `contents`, out-of-flow and
flex/grid-item blockification, anonymous block wrappers, table repair,
list-item markers, and inline continuations split around in-flow blocks.
Flex/grid blockification is parent-sensitive: each element child becomes its
own item, while only a text run may become an anonymous item. Tests preserve
`::before`, `::after`, and `::marker` origins through normalization.

Livery builds that input once from computed values and traverses the resulting
`BoxId` tree for layout. The old `LoweringSource`, `SuppressedId`, suppressed
subtree, and comment-boundary models are deleted. `display: none`,
`display: contents`, comments, anonymous boxes, and continuations now have box
generation rules instead of lowering exceptions.

The retained layout path now has one inline formatting context. Parley owns
whitespace processing, shaping, bidi, line breaking, inline atoms, and
baselines; Buckram receives the resulting per-line fragments. The retained
`TextFrame` is reused by paint, so paint does not shape that context again.
Paint also checks Buckram's formatting-context role before preparing a nested
inline group. This fixed a concrete disagreement where paint read a flex
item's DOM `display: inline-block`, re-entered inline layout, and applied its
margin twice.

The whole-document preliminary pass is deleted. `LegacyFragmentPlane`,
`LayoutPass`, and `LiveryLayout::atomic_fragments` are gone. Inline-block and
replaced subtrees still need isolated size measurements, but those measurements
are keyed by `BoxId`, retain their Buckram subtree fragments, and merge into
the final `FragmentTree`. There is no DOM-node-to-rectangle result map at that
boundary.

One source box producing multiple fragments is now executable behavior:
`retained_inline_format_is_not_shaped_again_for_paint` asserts a wrapped inline
owns at least two Buckram fragments, while
`split_inline_continuations_format_their_own_box_children` proves two generated
continuations retain distinct child runs around the intervening block.

General `::before` and `::after` box origins are accepted by the generator.
The live Livery integration currently emits `::marker`; generated content for
`::before` and `::after` still depends on the separate `content` longhand and
is not claimed here.

Verification:

- `cargo test -p buckram`: 15 passed.
- `cargo test -p livery -p genet-livery`: passed, including 3 retained-layout
  unit tests, 12 box-generation integration tests, and 48 paint tests.
- strict Clippy passed for Buckram and genet-livery. The workspace Clippy
  configuration still prints its existing unreachable-disallowed-type warning.
- exact-file `rustfmt --check` and `git diff --check`: passed.
- `cargo check -p genet-scripted -p genet-wpt`: passed.
- release `genet-wpt` expectation guards over the final source returned
  `unexpected=0` in all nine directories. One broad CSS2 run transiently moved
  `first-letter-punctuation-100.xht`; it passed immediately in isolation and
  the full CSS2 tie-breaker returned `unexpected=0`.

Exact reftest diff, frozen K1 baseline
`Code/testing/genet/wpt-ledger/2026-07-26_buckram_k1` against
`Code/testing/genet/wpt-ledger/2026-07-27_buckram_k2`:

| directory | K1 pass | K2 pass | delta | gains | regressions |
|---|---:|---:|---:|---:|---:|
| css-backgrounds | 218 | 218 | 0 | 0 | 0 |
| css-borders | 28 | 28 | 0 | 0 | 0 |
| css-flexbox | 348 | 369 | +21 | 38 | 17 |
| css-grid | 433 | 427 | -6 | 4 | 10 |
| css-multicol | 105 | 98 | -7 | 1 | 8 |
| css-position | 40 | 40 | 0 | 2 | 2 |
| css-tables | 59 | 50 | -9 | 2 | 11 |
| css-writing-modes | 224 | 225 | +1 | 6 | 5 |
| CSS2 | 4,289 | 4,159 | -130 | 28 | 158 |
| **total** | **5,744** | **5,614** | **-130** | **81** | **211** |

All 16,375 test URLs retain their K1 cardinality. One grid URL remains
`fail` but no longer crashes. The 81 gains include 38 flexbox files, both
`bidi-008` variants, and the 14 `block-in-inline-insert-*` reference files.

The losses are real and remain open:

- CSS2 contributes 158 regressions, concentrated in floats/clearance,
  normal flow, line boxes, margin/padding, tables, positioning, and borders.
  The K1 code had already measured a 131-file CSS2 loss when fake
  whitespace-only flow boxes were removed from the old emulation. K2's net
  CSS2 loss of 130 is consistent with that dependency, but is not treated as
  proof for every URL. K3 must replace the missing flow, margin-collapse,
  float, and line-box algorithms rather than restore boxes CSS says do not
  exist.
- The 11 table regressions are height distribution, collapsed-border paint
  order, baseline/static position, and percentage sizing. They stay with K4.
- The 17 flex regressions are named baseline, intrinsic/flex-basis, replaced
  aspect-ratio, percentage anonymous-item, and stacking/paint-order families.
  They stay with K3 and K5.
- Grid's 10 regressions are lanes, subgrid, intrinsic track sizing, baseline,
  and orthogonal/scrollbar cases. Multicol's 8 regressions remain non-evidence
  until K6 supplies fragmentation. Position's 2 and writing-mode's 5
  regressions remain K5 and K3 work.

The frozen K1 ledger therefore remains the conformance ratchet. K2's ledger is
an architectural receipt and an explicit debt map; it must not be relabelled
as an improved baseline.

### K3. Logical flow and intrinsic queries

Make inline and block axes primary. Implement intrinsic-size queries, BFC/IFC
establishment, margin collapsing, clearance, float exclusion, shrink-to-fit,
and the containing-block relationships required by flow.

K3m onward is executed through the
[K3 completion execution plan](2026-07-28_buckram_k3_completion_execution_plan.md).
That plan owns slice order and interim receipts. This architecture plan keeps
the K3 boundary and receives the final closure receipt.

This stage reviews each remaining use of Taffy's block algorithm. Keep a use
only when Buckram can supply its CSS inputs and recover correct fragments
without hiding required state.

**Receipt:** named css-writing-modes, css-sizing, CSS2 float/BFC, and
margin-collapse families; min-content and max-content differ in structural
fixtures; zero unexplained corpus regressions.

#### K3a receipt - 2026-07-27

K3a establishes the logical-axis and intrinsic-query contracts. It does not
close K3.

Buckram now owns `WritingMode`, `Direction`, `FlowAxes`, logical and physical
sizes, and the abstract-to-physical side mapping from CSS Writing Modes Level
4 section 6.4. All five writing modes accepted by Livery are represented,
including the distinct inline directions of `sideways-lr`. Every generated
`CssBox` carries its inherited flow axes, including text, pseudo, marker, and
anonymous fixup boxes.

Livery's `direction` longhand moved from the known-unimplemented catalog into
the computed-value model. It inherits independently of `writing-mode` and
crosses the Livery-to-Buckram adapter as `Direction`, rather than becoming a
backend flag.

`Fragment::from_logical` derives physical geometry from a logical rectangle,
its containing-block size, and its flow axes. The live Taffy migration path
still uses `from_horizontal_physical`; replacing that compatibility
constructor requires Buckram's block algorithm later in K3.

Intrinsic sizing is now an explicit query:

```rust
IntrinsicSizeQuery::new(box_id, LogicalAxis::Inline, IntrinsicSizeKind::MinContent)
```

`IntrinsicSizeCache` is keyed by `BoxId` and logical axis. One provider
measurement supplies a validated min-content/max-content pair, so the two
queries stay distinct without duplicating shaping. Backend node ids are not
part of the contract.

Still open in K3:

- wire the cache to Buckram-owned inline and block formatting contexts;
- replace the remaining Taffy block dispatch;
- implement margin collapsing, floats, clearance, shrink-to-fit, and BFC
  establishment;
- construct live vertical fragments from logical geometry.

Verification:

- `cargo test -p buckram`: 21 passed.
- `cargo test -p livery -p genet-livery`: passed.
- strict Clippy passed for Buckram and for genet-livery with dependency
  linting disabled. The separate Livery value crate still has its 147
  previously recorded warnings.
- exact-file `rustfmt --check` and `git diff --check`: passed.
- the release `css/css-writing-modes` reftest run remained at 225 passes and
  returned `unexpected=0` against the frozen K2 expectations.

#### K3b receipt - 2026-07-27

K3b wires the first intrinsic queries into the live formatting path and moves
formatting-context dispatch out of Taffy's input model. It does not close K3.

Every scratch node now carries a Buckram `AlgorithmKind`: hidden, leaf, block,
flex, or grid. `AlgorithmRun::compute_node` dispatches from that role instead
of reading `taffy::Style::display`. Livery selects the role from the generated
`CssBox` formatting context and table-internal role before it constructs the
private backend style. A structural test deliberately pairs
`AlgorithmKind::Flex` with `taffy::Display::Block` and still gets flex
placement, proving the backend enum no longer chooses the algorithm.

A retained inline leaf now has an owning `BoxId` when it represents that box's
whole IFC. When Taffy asks the leaf for min-content or max-content inline
size, Livery forms an `IntrinsicSizeQuery` for that box and logical axis. The
first query formats the minimum and maximum cases through Parley, validates
the pair, and stores it in Buckram's `IntrinsicSizeCache`; the other query
reuses the pair. The final line layout is formatted at the returned intrinsic
width, so fragment construction does not confuse the probe width with the
used width.

The current workaround can produce two partial inline leaves around an
out-of-flow child. Those leaves deliberately bypass the box-keyed cache rather
than aliasing two partial measurements under one parent `BoxId`. A structural
test freezes that guard. It remains until Buckram's owned IFC handles
out-of-flow static-position participants directly.

The remaining boundary is explicit: `AlgorithmKind::Block` still calls
Taffy's block algorithm, and Livery still constructs private Taffy styles.
K3c begins replacing that block call with Buckram's BFC implementation and
gives it logical margins, float and clearance inputs, and containing-block
state. Later K3 slices widen admission only as each shared BFC rule lands.

Verification:

- `cargo test -p buckram`: 22 passed.
- `cargo test -p genet-livery`: passed.
- strict Clippy passed for Buckram and for genet-livery with dependency
  linting disabled.
- the release all-nine expectation guard retained K2's 5,614 passes and
  returned `unexpected=0` in every directory:
- after the partial-inline cache guard landed, the release runner was rebuilt
  and the affected `css-position` and CSS2 ledgers were repeated.
  `css-position` remained at 40 passes with `unexpected=0`. The first CSS2 run
  reproduced the existing transient failure in
  `first-letter-punctuation-100.xht`; that file passed alone, and the full
  tie-breaker returned 4,159 passes with `unexpected=0`.

| directory | passes | unexpected |
|---|---:|---:|
| css-backgrounds | 218 | 0 |
| css-borders | 28 | 0 |
| css-flexbox | 369 | 0 |
| css-grid | 427 | 0 |
| css-multicol | 98 | 0 |
| css-position | 40 | 0 |
| css-tables | 50 | 0 |
| css-writing-modes | 225 | 0 |
| CSS2 | 4,159 | 0 |
| **total** | **5,614** | **0** |

#### K3c receipt - 2026-07-27

K3c proves a live Buckram-owned normal block-flow lane. It does not close K3,
and it does not claim that Taffy's block algorithm has disappeared.

Buckram now owns a CSS-facing `BlockStyle` beside the private backend style.
It carries the box's flow axes and its containing block's axes, preferred,
minimum and maximum sizes, physical margins, padding and borders, box sizing,
positioning, float and clearance, independent-BFC establishment, replaced and
aspect-ratio state, size containment, and a nonlinear-length marker. The
adapter therefore does not infer CSS roles from Taffy's style enum.

`BlockFormattingContext` performs logical block stacking. Its pure structural
fixtures cover:

- CSS 2.2 section 10.3.3's block-width equation, including two auto inline
  margins;
- adjoining positive and negative margin collapse as separate extrema;
- `vertical-rl` block progression from the physical right edge.

The live algorithm admits a context only when its inline size is definite, it
is horizontal normal flow, and the whole shared block subtree is static,
non-floating, margin-safe, non-replaced, and free of intrinsic keywords,
independent BFCs, aspect-ratio sizing, containment, and nonlinear math. Every
rejection is a named `BlockDeferral`. This is deliberately a whole-subtree
test: consuming the size returned by a deferred child is not enough when that
child also returns margin or float state shared with its ancestors.

Two receipts prove that this is not Taffy under a new name:

- a Buckram adapter fixture gives a child `width: 80px` while the private
  Taffy style says `10px`; the result is 80px and reports
  `BlockAlgorithm::Buckram`;
- a live HTML/Livery fixture lays out the root, `html`, `body`, and a nested
  block container with four Buckram block calls, zero Taffy block calls, and
  two children stacked at the expected 20px interval.

The first broad guard found one real regression,
`background-color-body-propagation-002.html`. A Buckram parent had consumed a
Taffy child's size while dropping the descendant margin-collapse state, so
the red body background extended behind a paragraph's collapsed margins.
Deferral now propagates through the shared block subtree. The isolated test
passed after that correction, and both the full backgrounds and CSS2 guards
returned to their frozen results.

Still open in K3:

- admit nonzero margins only after parent/child collapse-through and empty
  block rules are live in Buckram;
- model Livery's `clear` longhand, then own float exclusion and clearance;
- finalise auto block size before physical conversion in vertical flow;
- own relative/absolute/static-position state, shrink-to-fit, independent
  BFC baselines, intrinsic block queries, and the out-of-flow IFC participant
  currently protected by K3b's cache guard.

Verification:

- `cargo test -p buckram -p genet-livery`: passed; Buckram has 26 unit tests,
  and the complete genet-livery unit/integration/doc-test suite is green.
- strict Clippy passed for both crates with dependency linting disabled. The
  workspace configuration still prints its existing unreachable
  disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- the frozen K2 expectation guard passed on the final source:

| directory | passes | unexpected |
|---|---:|---:|
| css-backgrounds | 218 | 0 |
| css-borders | 28 | 0 |
| css-flexbox | 369 | 0 |
| css-grid | 427 | 0 |
| css-multicol | 98 | 0 |
| css-position | 40 | 0 |
| css-tables | 50 | 0 |
| css-writing-modes | 225 | 0 |
| CSS2 | 4,159 | 0 |
| **total** | **5,614** | **0** |

#### K3d receipt - 2026-07-27

K3d admits collapsing margins into the live Buckram block lane. It does not
close K3.

Buckram now carries collapsed block margins as output state rather than
recovering them from a backend rectangle. Positive and negative margins keep
separate extrema. The owned BFC collapses adjoining siblings, a first or last
child with its parent when the CSS separators permit it, and an empty block's
start and end margins through to the surrounding chain. Borders, padding,
non-auto block size, positive minimum block size, a root box, and an
independent BFC stop the corresponding collapse. The root element's own
margins never collapse.

The block-width solver also implements CSS 2.2 section 10.3.3's negative
overconstraint rule: when `margin-inline-start` is the sole auto margin and
the remaining width is negative, its used value is zero and the inline-end
margin absorbs the excess. These rules come from CSS 2.2's
[collapsing-margin model](https://www.w3.org/TR/CSS22/box.html#collapsing-margins)
and [normal-flow block-width equation](https://www.w3.org/TR/CSS22/visudet.html#blockwidth),
not from Taffy's output.

Margin state propagates recursively through `AlgorithmTree`. A deferred child
still defers the whole shared margin subtree; Buckram does not consume its
rectangle and discard its unresolved float, clearance, positioning, or
intrinsic state. The live fixture exercises parent/child collapse, sibling
collapse, a negative empty-block chain, and reports at least six Buckram block
calls with zero Taffy block calls.

The slice also corrected four evidence boundaries exposed by the broad run:

- both synthetic layout roots now represent the definite initial containing
  block, so an `html` -> `body` -> descendant `height: 100%` chain resolves
  against the viewport while remaining entirely in Buckram;
- HTML `width` and `height` hints retain percentage values, CSS dimensions
  take precedence, and `canvas` joins `img` as replaced content;
- the isolated atomic-inline prepass admits only a generated box whose outer
  display role is inline, so blockified flex/grid items and positioned
  replaced boxes are not measured against the viewport as inline leaves;
- the WPT renderer now composites a transparent CSS canvas over an opaque
  white browser backdrop. Canvas-background propagation remains engine-owned,
  including CSS Backgrounds 3 section 2.11's rule that a selected
  `display: none` source leaves the canvas transparent. The UA sheet also
  restores `blockquote`'s block role and conventional margins.

The same Livery cleanup added the computed `contain` model needed to suppress
HTML body-background propagation and completed the retained single-animation
lane for keyframe properties and negative delays. Those changes explain some
corpus gains below; they are not Buckram layout claims.

The backdrop correction means K2 is not a valid per-file ratchet for
canvas-sensitive cases. The following K2-to-K3d diff is an exact debt map, not
a claim that all gains came from the block algorithm or that K3d should replace
the frozen conformance baseline:

| directory | K2 pass | K3d pass | delta | gains | regressions |
|---|---:|---:|---:|---:|---:|
| css-backgrounds | 218 | 241 | +23 | 24 | 1 |
| css-borders | 28 | 32 | +4 | 4 | 0 |
| css-flexbox | 369 | 380 | +11 | 17 | 6 |
| css-grid | 427 | 433 | +6 | 7 | 1 |
| css-multicol | 98 | 98 | 0 | 1 | 1 |
| css-position | 40 | 41 | +1 | 1 | 0 |
| css-tables | 50 | 50 | 0 | 0 | 0 |
| css-writing-modes | 225 | 225 | 0 | 0 | 0 |
| CSS2 | 4,159 | 4,230 | +71 | 93 | 22 |
| **total** | **5,614** | **5,730** | **+116** | **147** | **31** |

The 31 losses are named and remain assigned:

- `css3-background-size-001` remains a background-image percentage-sizing
  discrepancy;
- the six flex losses are the two vertical-canvas files, the two
  `flex-basis: content` files, the whitespace `001b` file, and
  `flexbox_quirks_body`. The last requires the WHATWG
  [body-fills-html quirk](https://quirks.spec.whatwg.org/#the-body-element-fills-the-html-element-quirk);
- `grid-baseline-align-cycles-001` requires an `inline-grid` outer/inner
  display distinction plus cyclic baseline sizing;
- `multicol-height-002-print` remains K6 fragmentation work;
- CSS2's two `abspos-containing-block-initial-009*` files and
  `height-percentage-003` now lay out the test side correctly but still have
  positioned percentage references, which remain K5 work;
- `line-height-oof-descendants-001` is K3b's named out-of-flow IFC
  participant gap, while `height-percentage-005` is the remaining percentage
  replaced-height case inside an auto-height containing block;
- CSS2's four border files and twelve explicit or inherited
  `height`/`min-height`/`max-height` files were false passes against the old
  black backdrop. White exposes their existing geometry differences;
- CSS2's legacy `root-box-003` expects a green background from a
  `display: none` root. The current CSS Backgrounds rule requires a transparent
  canvas, so this is an intentional standards correction rather than a
  conformance regression.

`clear` is now a computed Livery longhand and a Buckram `ClearSide` input.
Non-`none` values produce the named `Clearance` deferral. Float exclusion and
clearance are not claimed.

Verification:

- `cargo test -p buckram -p genet-livery`: 32 Buckram tests and 128
  genet-livery tests passed.
- strict Clippy passed for both crates with dependency linting disabled. The
  workspace configuration still prints its existing unreachable
  disallowed-type warning.
- exact-file `rustfmt --check` and `git diff --check`: passed.
- the feature-unified release `genet-wpt` build passed. Fresh expectations
  for all 16,375 URLs are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3d`.

Still open in K3: float exclusion and clearance, vertical auto block-size
finalisation, positioning and shrink-to-fit, independent-BFC baselines,
intrinsic block queries, and the out-of-flow IFC participant protected by
K3b's cache guard.

#### K3e receipt - 2026-07-27

K3e lands the first standards-owned float and clearance lane. It does not
close floats or K3.

Float is now a box-generation input. A floated inline-level principal box is
blockified before anonymous-box construction, retains its physical left or
right role, and is excluded from both the in-flow block and in-flow inline
runs. Livery therefore cannot swallow a float into the text leaf merely
because its computed outer display was inline.

`BlockFormattingContext` now owns float margin-box exclusions. For a
definite-width direct float it:

- resolves automatic margins to zero under CSS 2.2 section 10.3.5;
- starts at the hypothetical normal-flow position without advancing the
  normal block cursor;
- places the margin box at the highest available position, then at the
  requested physical left or right side;
- lowers it to the next overlapping float bottom when the available interval
  is too narrow;
- applies physical `clear: left | right | both` against the corresponding
  lowest float margin edge;
- inhibits parent-start margin collapse when clearance is actually
  introduced; and
- includes the lowest float margin edge in the auto height of the BFC that
  contains it.

These are implementations of CSS 2.2's
[float placement and clearance rules](https://www.w3.org/TR/CSS22/visuren.html#floats)
and [float width equation](https://www.w3.org/TR/CSS22/visudet.html#float-width),
not a reconstruction of Taffy's result. `FloatAvailableSpace` exposes the
remaining flow-relative interval for a future line breaker. It is not yet
fed to Parley.

Admission is intentionally narrower than the data model. Buckram accepts an
independent BFC only when it directly owns this float lane. Auto-width floats,
float-affected line boxes, an atomic BFC that must avoid an active float,
float or clearance state crossing an ordinary nested block, and clearance
through a collapsed empty box have separate `BlockDeferral` values. Other
independent BFCs retain K3d dispatch.

That last guard was learned from evidence. A preliminary build admitted every
independent BFC and moved 153 all-nine statuses, including 40 losses. That
build was rejected. The accepted build admits only a direct definite-float
scenario. Its live fixture has a 200px overflow BFC, a blockified 80x40 left
float, a 60x70 right float, and a 10px `clear: both` block. The two floats
share the first row at x=0 and x=140, the clear block starts at y=70, the BFC
has an 80px auto height, and the document reports four Buckram block calls
with zero Taffy block calls.

The final K3d-to-K3e all-nine diff is exact across all 16,375 URLs:

| directory | K3d pass | K3e pass | delta | gains | regressions |
|---|---:|---:|---:|---:|---:|
| css-backgrounds | 241 | 241 | 0 | 0 | 0 |
| css-borders | 32 | 32 | 0 | 0 | 0 |
| css-flexbox | 380 | 384 | +4 | 4 | 0 |
| css-grid | 433 | 435 | +2 | 2 | 0 |
| css-multicol | 98 | 97 | -1 | 0 | 1 |
| css-position | 41 | 41 | 0 | 0 | 0 |
| css-tables | 50 | 50 | 0 | 0 | 0 |
| css-writing-modes | 225 | 226 | +1 | 1 | 0 |
| CSS2 | 4,230 | 4,227 | -3 | 8 | 11 |
| **total** | **5,730** | **5,733** | **+3** | **15** | **12** |

The losses are not hidden by the positive total. Nine directly touch float
structure that K3e now represents correctly while later algorithms remain
deferred:

- `multicol-span-float-003`;
- `c414-flt-wrap-001`;
- `float-after-block-with-collapsed-margin-inside-inline`;
- `float-in-inline-anonymous-block-with-overflow-hidden`;
- `float-nowrap-4` and `float-nowrap-8`;
- `floats-placement-vertical-001a`;
- `positioning-float-001`; and
- `white-space-processing-048`.

They require line-box exclusion, nested anonymous-block float propagation,
vertical float placement, positioning, or multicol fragmentation. Restoring
the old inline grouping would recover false passes by undoing CSS Display's
float role, so it is not an acceptable fix.

The other three losses, `c5505-mrgn-003`, `margin-collapse-004`, and
`root-box-001`, contain no float. They are stable on the tie-breaker but
cannot be causally assigned to K3e in this already-dirty shared source tree.
They remain named corpus debt rather than being presented as float evidence.
For the same reason, the 15 gains are whole-source movement, not all claimed
as Buckram float wins.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  38 Buckram tests, 129 genet-livery tests, 141 Livery tests, and all doc
  tests green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- fresh expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3e_final`. Each of the
  five moved directories was rerun against that ledger and returned
  `unexpected=0`.

Still open in K3: per-line float exclusion using variable Parley break widths,
atomic-BFC avoidance beside floats, shrink-to-fit and intrinsic float widths,
float state through ordinary nested blocks, collapsed-empty-box clearance,
vertical auto block-size finalisation, positioning, independent-BFC
baselines, intrinsic block queries, and the out-of-flow IFC participant
protected by K3b's cache guard.

#### K3f receipt - 2026-07-27

K3f admits one real float-affected inline formatting lane. It does not close
floats or K3.

Buckram now snapshots the active float margin boxes into an immutable
`FloatLineConstraints` value for a direct measured inline leaf. The query is
flow-relative and takes the line's complete block-axis span. Its physical
adapter handles horizontal RTL without making physical coordinates primary.
It also exposes the next lower float boundary that gives an overfull line more
inline room.

The scratch algorithm boundary carries those constraints separately from
known and available size. A measured leaf must opt in explicitly. Livery's
retained, soft-wrapping inline formatter opts in; the deterministic estimate
callback, `nowrap`, and nested line contexts do not, so they retain the named
`FloatLineExclusion` or `NestedFloatState` deferral rather than silently
claiming support. Float geometry is not part of Taffy's cache key, so the
admitted leaf's backend cache is cleared for the final BFC pass. Livery's own
retained inline cache is keyed by both width and the Buckram constraint
snapshot, preventing a scalar intrinsic probe from aliasing the final
float-aware layout.

Parley 0.10 already supplies the needed shaped-paragraph primitive. Before
each line, Livery sets that line's x offset and maximum advance from Buckram's
interval. After Parley reports the actual line height, Livery queries the full
line span and reverts and rebreaks the line if a float boundary changed the
interval. If no content fits beside the float, it retries at the next wider
float boundary and updates the line's block position. Alignment then runs
inside each line's own box. This is not a port of the incumbent's
top-edge-only band lookup.

The live receipt uses a 200px independent BFC, an 80x40 left float, and a
20px-line-height paragraph. Its first two line fragments start at x=80; every
line at or below y=40 starts at x=0 and reclaims the full column. The root,
html, body, and host remain in Buckram with zero Taffy block calls.

The final all-nine diff has two status changes, both direct float evidence and
both gains:

- `css/CSS2/floats/floats-zero-height-wrap-001.xht`: fail -> pass;
- `css/CSS2/floats/floats-zero-height-wrap-002.xht`: fail -> pass.

Those tests are the useful distinction. A line spans a cleared 1px or
zero-height float boundary; looking only at the line's top would miss the
exclusion. The full-span query causes a rebreak against the 100px inset.

The frozen K3e JSON contains 41 css-position passes, although K3e's printed
table said 40; its stated 5,733 total was correct. Comparing frozen JSON to
frozen JSON gives:

| directory | K3e pass | K3f pass | delta | gains | regressions |
|---|---:|---:|---:|---:|---:|
| css-backgrounds | 241 | 241 | 0 | 0 | 0 |
| css-borders | 32 | 32 | 0 | 0 | 0 |
| css-flexbox | 384 | 384 | 0 | 0 | 0 |
| css-grid | 435 | 435 | 0 | 0 | 0 |
| css-multicol | 97 | 97 | 0 | 0 | 0 |
| css-position | 41 | 41 | 0 | 0 | 0 |
| css-tables | 50 | 50 | 0 | 0 | 0 |
| css-writing-modes | 226 | 226 | 0 | 0 | 0 |
| CSS2 | 4,227 | 4,229 | +2 | 2 | 0 |
| **total** | **5,733** | **5,735** | **+2** | **2** | **0** |

The one CSS2 `error` status is byte-for-byte unchanged from K3e and is not
counted as movement.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  40 Buckram tests, 130 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- fresh expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3f`; the complete CSS2
  repeat returned `unexpected=0`.

Still open in K3: atomic-BFC avoidance beside floats, shrink-to-fit and
intrinsic float widths, float state through ordinary nested blocks, nowrap and
nested inline contexts beside floats, collapsed-empty-box clearance, vertical
auto block-size finalisation, positioning, independent-BFC baselines,
intrinsic block queries, and the out-of-flow IFC participant protected by
K3b's cache guard.

#### K3g receipt - 2026-07-27

K3g admits the first block-level independent formatting context beside active
floats. It does not close every BFC or float-avoidance case.

CSS2 requires the border box of an in-flow element that establishes a new BFC
not to overlap float margin boxes in the same BFC. It permits the element to
sit beside the floats when space is sufficient or move below them, and
deliberately leaves the amount and timing of any border-box narrowing
undefined. Buckram now owns an explicit policy for the admitted lane:

- `width: auto` is solved inside the available float band while percentages
  continue to resolve against the actual containing block;
- a definite or minimum border-box width that cannot fit moves to the next
  overlapping float boundary;
- the available band is queried across the candidate's complete border-box
  block span, not only at its top edge;
- the isolated subtree is measured again when the candidate width changes,
  and the query repeats with the measured block size until the width is
  stable; and
- the chosen logical placement is committed to normal block flow, including
  any downward displacement and the resulting BFC auto height.

This is an explicit scratch-tree capability. Livery opts in only a static,
non-floating, non-replaced, horizontal block-level `flow` or `flow-root`
principal box whose algorithm is block or leaf and whose inline margins are
explicit zeroes. Flex, grid, tables, atomic inline boxes, orthogonal flow, and
nonzero or automatic inline margins retain their named deferrals. That guard
keeps CSS2's implementation-policy freedom from becoming an accidental
backend behavior.

Taffy's cache does not include the active float band. An earlier full-width
root or intrinsic probe can therefore satisfy a later candidate-width query
without re-entering the measured leaf. K3g clears the isolated BFC subtree's
backend caches before each candidate measurement. Livery's retained inline
cache remains width-keyed, so shaped content is then formatted at the actual
candidate width.

The pure fixture uses a 200px BFC and an 80x40 left float. An auto-width BFC
becomes 120px wide at x=80 and y=0. A 150px definite-width BFC cannot fit in
that band and moves to x=0 and y=40. The live HTML/Livery fixture proves the
same two placements in one host, gives the host a 60px auto height, and
reports zero Taffy block calls.

This slice has no WPT status movement. That is recorded rather than promoted
into a conformance claim:

- the focused `css/CSS2/floats` repeat remains 44 passes, 57 failures, 43
  skips, and zero errors across 144 files;
- the complete CSS2 repeat remains 4,229 passes, 1,745 failures, 3,279 skips,
  and one error across 9,254 files; and
- every frozen all-nine K3f expectation guard returns `unexpected=0`, so the
  total remains 5,735 with zero gains and zero regressions.

The classic `floats-wrap-bfc-*` set is not evidence for this narrow lane yet.
Its table-hosted variants enter through K4's table parent, while the
margin-policy and other atomic variants remain outside K3g's explicit
admission. The structural and live fixtures are therefore the acceptance
receipt; WPT remains the unchanged regression guard.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  42 Buckram tests, 131 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- fresh focused and complete CSS2 expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3g`.

Still open in K3: nonzero-inline-margin and non-block BFC avoidance beside
floats, shrink-to-fit and intrinsic float widths, float state through ordinary
nested blocks, nowrap and nested inline contexts beside floats,
collapsed-empty-box clearance, vertical auto block-size finalisation,
positioning, independent-BFC baselines, intrinsic block queries, and the
out-of-flow IFC participant protected by K3b's cache guard.

#### K3h receipt - 2026-07-27

K3h admits the first intrinsic shrink-to-fit float shape. The admitted box is
a static, horizontal, non-replaced block-level `flow` or `flow-root` float
with exactly one measured inline formatting context. Livery marks that shape
explicitly; `BlockStyle::shrink_to_fit` alone does not silently route a node
through the new path.

Buckram now asks the inline formatting context for min-content and max-content
widths through the existing measure boundary. Livery answers those requests
through its owner-and-axis `IntrinsicSizeCache`, so one intrinsic pair is
retained independently of any definite-width line layout. Buckram then owns
the CSS2 section 10.3.5 calculation:

`min(max(min-content, available), max-content)`

Available width is the containing block's inline size after the float's used
margins, border, and padding. Automatic inline margins resolve to zero.
Padding and border are added to the selected content width, after which
section 10.4's constraint order is applied: `max-width` first, then
`min-width`. The latter therefore wins when an author supplies conflicting
constraints. Taffy receives the resolved width; it does not infer the
intrinsic pair or choose the float sizing rule.

The admission is deliberately narrower than the formula's eventual use.
Multi-child block content, replaced floats, inline-block shrink-to-fit,
tables, positioned floats, orthogonal flow, and boxes without a retained
inline measure context keep their existing named deferrals.

The pure solver fixture uses min-content 40px, max-content 120px, and 10px of
inline padding. It resolves to 100px in a 100px containing block, 130px in a
200px containing block, and the 50px minimum outer size in a 30px containing
block. A conflicting 90px `min-width` and 60px `max-width` resolves to the
100px minimum border box. The adapter fixture records min-content,
max-content, and definite-width measure requests and keeps both the float and
its root in Buckram. The live Livery fixture proves that an auto float fills
an 80px host and wraps, while the same content stops at max-content in a
200px host; the receipt reports zero Taffy block calls.

This slice has no WPT status movement:

- the focused `css/CSS2/floats` repeat remains 44 passes, 57 failures, 43
  skips, and zero errors across 144 files;
- the complete CSS2 ledger is byte-identical to K3g at 4,229 passes, 1,745
  failures, 3,279 skips, and one error across 9,254 files; and
- every frozen all-nine K3f expectation guard returns `unexpected=0`, so the
  total remains 5,735 with zero gains and zero regressions.

The structural and live fixtures are the capability receipt. The unchanged
WPT ledger is a regression guard, not evidence that the remaining float
families are implemented.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  44 Buckram tests, 132 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- fresh focused and complete CSS2 expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3h`.

Still open in K3: multi-child and block-content auto floats, inline-block
shrink-to-fit, nonzero-inline-margin and non-block BFC avoidance beside
floats, float state through ordinary nested blocks, nowrap and nested inline
contexts beside floats, collapsed-empty-box clearance, vertical auto
block-size finalisation, positioning, independent-BFC baselines, intrinsic
block queries, and the out-of-flow IFC participant protected by K3b's cache
guard.

#### K3i receipt - 2026-07-27

K3i closes clearance through a self-collapsing empty block. The
`ClearanceThroughCollapsedBox` deferral has been removed.

CSS2 gives this case two linked rules. When clearance is introduced, it
separates the box from the preceding margin chain and places the box's border
edge past the relevant float. If the empty box's own block-start and
block-end margins are adjoining, they may still collapse through into the
top margin of a following sibling. That resulting chain must not collapse
with the parent box's block-end margin.

`BlockFormattingContext` now records whether its active trailing collapsed
margin contains clearance. The flag belongs to the active margin chain, not
to the parent or child box generally:

- a cleared self-collapsing box starts a new active chain at its used border
  position;
- later self-collapsing siblings extend that chain;
- a non-collapsing box consumes the chain and replaces it with its own
  block-end margin; and
- only a still-active cleared chain suppresses parent-end margin collapse.

This avoids both failures available to a coarse boolean: letting the cleared
chain escape through the parent, or permanently disabling valid parent-end
collapse after a later non-empty sibling.

The pure fixture uses a 200px BFC, an 80x40 left float, an empty `clear:
left` box with 10px and 20px block margins, and a following 10px box with a
30px block-start margin. The empty border box lands at y=40, its margins
collapse with the following margin, the next box lands at y=70, and used
block size is 80px. A trailing-empty variant retains a 60px used size and
reports that its active margin cannot collapse with the parent end.

The adapter fixture proves the same y=40 and y=70 placements, 80px root
height, and Buckram dispatch for both the empty box and root. The live
HTML/Livery fixture repeats that geometry inside an explicit overflow BFC and
reports zero Taffy block calls.

This slice has no WPT status movement:

- `css/CSS2/floats-clear` remains 65 passes, 146 failures, 38 skips, and zero
  errors across 249 files;
- `css/CSS2/floats` remains 44 passes, 57 failures, 43 skips, and zero errors
  across 144 files;
- the complete CSS2 ledger is byte-identical to K3h at 4,229 passes, 1,745
  failures, 3,279 skips, and one error across 9,254 files; and
- every frozen all-nine K3f expectation guard returns `unexpected=0`, so the
  total remains 5,735 with zero gains and zero regressions.

The closest public self-collapsing clearance tests also enter positioned
subtrees or require float state to cross ordinary nested blocks. Those
deferrals remain real, so the structural and live fixtures are the capability
receipt and WPT remains the unchanged regression guard.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  46 Buckram tests, 133 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning.
- the feature-unified release `genet-wpt` build passed.
- fresh `floats`, `floats-clear`, and complete CSS2 expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3i`.

Still open in K3: multi-child and block-content auto floats, inline-block
shrink-to-fit, nonzero-inline-margin and non-block BFC avoidance beside
floats, float state through ordinary nested blocks, nowrap and nested inline
contexts beside floats, vertical auto block-size finalisation, positioning,
independent-BFC baselines, intrinsic block queries, and the out-of-flow IFC
participant protected by K3b's cache guard.

#### K3j receipt - 2026-07-27

K3j closes nonzero fixed inline margins for block-level BFC roots beside
floats and makes `display: flow-root` an actual Livery computed value.

CSS2 section 9.5 requires the border box of a block formatting context beside
a float not to overlap the float's margin box. It does not require the BFC
root's margin box to avoid the float. Buckram therefore resolves the ordinary
block width equation, including inline margins, inside each candidate float
band and checks the resulting border box. If a fixed margin places that border
box outside the available band, layout advances to the next float bottom and
resolves the same equation again against the recovered containing block.
Percentage inputs remain relative to the containing block, not the temporary
float band.

The pure fixture uses a 100px BFC, a 50x40 right float, and a 60px-high BFC
root with a 51px inline-start margin. The BFC border box cannot fit beside the
float, so it lands at x=51, y=40 with a 49px used width. The adapter fixture
proves the same placement, the 100px root height, and a 49px measure
constraint with Buckram block dispatch.

The live HTML/Livery fixture repeats the case in both directions:

- LTR uses a right float and `margin-left: 51px`;
- RTL uses a left float and `margin-right: 51px`;
- both `display: flow-root` boxes land at y=40 with a 49x60 border box; and
- the receipt reports zero Taffy block calls.

This slice also corrected a standards-model omission. Livery's `Display` enum
now accepts `flow-root`, and genet-livery maps it to block outer display plus
flow-root inner display. It establishes a BFC through Buckram instead of
silently falling back to `display: block`.

The focused `css/CSS2/floats` ledger moves from 44 to 47 passes, with these
three exact fail-to-pass changes:

- `floats-placement-004.html`;
- `floats-wrap-bfc-with-margin-004.html`; and
- `floats-wrap-bfc-with-margin-005.html`.

The complete CSS2 ledger has the same three gains and no regressions: 4,232
passes, 1,742 failures, 3,279 skips, and one error across 9,254 files. Seven
of the other eight frozen directories are byte-identical to K3f.
`css-multicol` moves from 97 to 96 passes because
`multicol-nested-029.html` loses a false pass: its reference now correctly
establishes a `flow-root` BFC, while the test's `columns: 1` container still
falls through ordinary block layout because multicol computed values and
fragmentation remain deferred. The tested page did not regress; correcting
the reference exposed that named multicol boundary. Across all nine
directories the net is two passes, from 5,735 to 5,737.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  48 Buckram tests, 134 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled. The workspace configuration still prints its existing
  unreachable disallowed-type warning. Livery's pre-existing strict-Clippy
  backlog was not part of this receipt.
- the feature-unified release `genet-wpt` build passed.
- fresh focused, complete CSS2, and all-nine expectations are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3j`.

Still open in K3: multi-child and block-content auto floats, inline-block
shrink-to-fit, automatic inline margins and non-block BFC avoidance beside
floats, float state through ordinary nested blocks, nowrap and nested inline
contexts beside floats, vertical auto block-size finalisation, positioning,
independent-BFC baselines, intrinsic block queries, and the out-of-flow IFC
participant protected by K3b's cache guard.

#### K3k receipt - 2026-07-27

K3k admits block-level flex and grid formatting-context roots beside an active
preceding float. Buckram owns the CSS float exclusion and normal-flow
placement. Taffy remains the algorithm library for the isolated flex or grid
subtree at the width Buckram supplies.

This is not a general widening of Buckram's block lane. A first attempt marked
every block-level flex and grid root as float-capable, which also let ordinary
parents adopt those independent formatting contexts when no float existed.
The `css-flexbox` guard caught the resulting unrelated one-gain,
one-regression swap. The final admission rule is causal:

- block and leaf BFC roots retain K3g's existing explicit lane;
- flex and grid roots bypass the independent-formatting-context deferral only
  while a preceding left or right float remains active; and
- a flex or grid root with no active float keeps the existing Taffy block
  parent path.

The adapter fixture uses a 100px BFC and a 40x40 left float. An auto-width,
20px-high flex root is laid out at x=40, y=0 with a 60px width. A following
70x20 grid root cannot fit in the same float band and moves to x=0, y=40.
Their 20x10 children retain exact flex and grid placements, the root height is
60px, and the parent block algorithm is Buckram. A counter-fixture removes
the float and proves the flex parent remains on the existing Taffy block lane.

The live HTML/Livery fixture proves the same flex and grid geometry, child
placements, 60px host height, Buckram block dispatch, and zero Taffy block
fallback calls. The flex and grid algorithms themselves remain Taffy calls;
the counter records only block-algorithm ownership.

This slice has no WPT status movement:

- focused `css/CSS2/floats` remains 47 passes, 54 failures, 43 skips, and zero
  errors across 144 files;
- complete CSS2 remains 4,232 passes, 1,742 failures, 3,279 skips, and one
  error across 9,254 files;
- `css-flexbox` remains 384 passes, 502 failures, 472 skips, and zero errors;
  and
- every frozen all-nine K3j expectation guard returns `unexpected=0`, so the
  total remains 5,737.

The adapter and live fixtures are the capability receipt. The selected WPT
corpus does not isolate a block-level flex or grid BFC beside a float, so the
unchanged ledgers remain regression guards rather than conformance evidence
for this lane.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  50 Buckram tests, 135 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled.
- the feature-unified release `genet-wpt` build passed.
- release-build and complete CSS2 guard logs are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3k`.

Still open in K3: multi-child and block-content auto floats, inline-block
shrink-to-fit, automatic inline margins, table and atomic-inline BFC
avoidance beside floats, float state through ordinary nested blocks, nowrap
and nested inline contexts beside floats, vertical auto block-size
finalisation, positioning, independent-BFC baselines, intrinsic block
queries, and the out-of-flow IFC participant protected by K3b's cache guard.

#### K3l receipt - 2026-07-27

K3l gives an explicitly admitted ordinary descendant block access to the same
CSS float context as its parent. Buckram owns that continuation. Taffy still
receives isolated flex and grid calls and has no float-context state.

The continuation is geometry, not a backend-tree shortcut:

- ancestor exclusions are translated from the parent content box into the
  descendant content box;
- the descendant exports only floats it created, not the inherited exclusions;
- exported floats are translated back into the parent content box before
  following siblings are placed;
- explicit BFC roots start with empty state and export nothing; and
- direction or writing-mode changes, positioned or replaced boxes, and
  unresolved generated-box roles remain outside the admission rule.

Collapsed margins make the descendant content origin an output of layout, so
the adapter cannot translate the float state once and assume it is correct. It
first lays out the child to obtain its margin state, predicts the content
origin, and repeats with translated exclusions until the origin stabilises.
The pass count is bounded by the number of exclusions; failure to converge is
an explicit `NestedFloatState` deferral.

The live admission is narrower than the algorithm fixture. A box must be a
static, non-replaced, same-flow block-level box which does not establish a BFC
or carry an internal-table role. Generated `Block` formatting-context roots
remain deferred because split inline continuations do not yet preserve enough
float provenance through box fixup. Floats still known to originate under an
inline box are marked and keep the subtree deferred. Potentially negative
block margins on an exported float are also deferred because their margin-box
extent is not yet safe to translate.

Two guard failures made those boundaries concrete. The first broad live gate
moved four focused float files: two gains and two regressions in the
`floats-placement-vertical-001*` split-inline family. A generated-box role
matrix showed that excluding `Block` formatting-context roots retained
`floats-placement-005.html` and removed both regressions. The first complete
CSS2 run then caught `floats-clear/margin-collapse-135.xht`: treating a nested
clear-only subtree as if it exported floats had widened Buckram into an
unsupported negative-margin collapse case. Nested clearance now requires the
same explicit shared-float role; its adapter counter-fixture and the WPT both
retain the old Taffy path.

The capability fixtures cover all three boundaries:

- the pure block fixture translates an ancestor left exclusion into a
  descendant, creates a right float there, and imports only that new exclusion
  back into the parent; its margin guard also rejects a potentially negative
  exported float;
- adapter fixtures prove nested-float export, inherited per-line constraints,
  inherited clearance, the BFC stop, split-inline deferral, and nested-clear
  deferral without the shared role; and
- live Livery fixtures prove that a float inside one admitted ordinary wrapper
  clears an outer sibling, that a `flow-root` stops the state, and that lines
  inside an admitted wrapper use the ancestor float band and reclaim the full
  width below it. Both live fixtures report zero Taffy block fallbacks.

Final WPT movement is one fail-to-pass and zero regressions:

- focused `css/CSS2/floats` moves from 47 passes and 54 failures to 48 passes
  and 53 failures, with 43 skips and zero errors across 144 files;
- the only changed file is `css/CSS2/floats/floats-placement-005.html`;
- complete CSS2 moves from 4,232 passes and 1,742 failures to 4,233 passes and
  1,741 failures, with 3,279 skips and one error across 9,254 files;
- the other eight frozen directories have no status movement; and
- the all-nine pass total moves from 5,737 to 5,738.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: passed with
  58 Buckram tests, 136 genet-livery tests, 141 Livery tests, and all doc tests
  green.
- strict Clippy passed for Buckram and genet-livery with dependency linting
  disabled.
- exact-file Rustfmt and `git diff --check` passed.
- the feature-unified release `genet-wpt` build passed.
- the role matrix, focused and complete CSS2 ledgers, all-nine ledgers, and
  release-build log are at
  `Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3l`.

Still open in K3: multi-child and block-content auto floats, inline-block
shrink-to-fit, automatic inline margins, non-table atomic-inline BFC
avoidance beside floats, float continuation through generated `Block`
formatting-context roots and across flow changes, negative-margin and
split-inline nested floats, nowrap and nested inline contexts beside floats,
vertical auto block-size finalisation, independent-BFC baselines, intrinsic
block queries, and the final Taffy block-dispatch audit.

Table-specific avoidance is routed to K4. Positioning and the out-of-flow IFC
participant protected by K3b's cache guard are routed to K5.

### K4. CSS tables

Implement anonymous table fixup, row and column structure, spans, fixed and
auto sizing, captions, separate and collapsed borders, and positioned table
parts. A Taffy grid call may solve track constraints after Buckram has run the
CSS table algorithm. Grid auto-sizing is not the table algorithm.

**Receipt:** carry forward the old B3a-c family ledger; remove the
positioned-row flattening guard and the partial `table-layout` marker only
when their named limitations are closed.

### K5. Positioning and incremental relayout

Implement static-position calculation, relative offsets, absolute and fixed
containing blocks, sticky constraints, overflow, and dirty-subtree relayout.

**Receipt:** named css-position families, fragment-level containing-block
fixtures, and a dirty-subtree receipt that leaves unaffected fragment
identities stable.

### K6. Fragmentation

Start with inline fragmentation already seeded at K2. Add block break tokens,
fragmentainers, multicol, and then pagination. A box continued across two
columns is the first load-bearing multi-fragment acceptance case.

Flex and grid fragmentation are a separate sub-gate under the fork policy
above.

**Receipt:** fragment-tree assertions for continuation, containing fragment,
and coordinate space; named css-multicol families; paint, hit testing,
accessibility, and CSSOM all consume the continued fragments.

## Cutover and deletion

- F4 still asks whether Livery can replace Stylo on the selected corpus.
- F5 may delete `genet-layout`, `stylo_taffy`, and the Stylo family only after
  the existing cutover receipts pass.
- F5 does not delete Taffy merely because Stylo is gone. Buckram owns that
  dependency and its fork ledger.
- `genet-layout` remains an oracle during the differential period, never a
  source of browser semantics. Lifted code must be re-expressed through
  Buckram's box and fragment contracts.
- Once fragment consumers have moved, delete `FragmentPlane`,
  `BoxFragmentPlane`, and every old node-to-rect compatibility path.

## Stop rules

- Stop a slice that can represent a CSS distinction only by collapsing it
  onto a backend enum. Extend the Buckram model first.
- Stop a fragment slice if paint, hit testing, accessibility, or CSSOM would
  keep reading only a principal rectangle.
- Stop an algorithm lift if it imports Stylo computed-value types or Servo
  tree ownership.
- Stop a Taffy patch that changes generic algorithm behavior without a focused
  upstream-shaped fixture and a patch-log entry.
- Do not claim a formatting context from a WPT count alone. Each context needs
  a structural fixture that names the box and fragment relationships.

## Done condition

Buckram is the only CSS layout engine in Genet. Livery supplies computed
values; Buckram owns box generation, formatting contexts, intrinsic sizing,
logical geometry, and a one-to-many `FragmentTree`; Taffy is reachable only
through the low-level flex/grid adapter; every fragment consumer uses fragment
identity; the old fragment planes and two-pass inline workaround are deleted;
and Stylo retirement leaves no gap in the standards-owned layout model.
