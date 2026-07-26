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

### K3. Logical flow and intrinsic queries

Make inline and block axes primary. Implement intrinsic-size queries, BFC/IFC
establishment, margin collapsing, clearance, float exclusion, shrink-to-fit,
and the containing-block relationships required by flow.

This stage reviews each remaining use of Taffy's block algorithm. Keep a use
only when Buckram can supply its CSS inputs and recover correct fragments
without hiding required state.

**Receipt:** named css-writing-modes, css-sizing, CSS2 float/BFC, and
margin-collapse families; min-content and max-content differ in structural
fixtures; zero unexplained corpus regressions.

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
