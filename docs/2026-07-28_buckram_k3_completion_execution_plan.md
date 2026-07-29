# Buckram K3 completion execution plan

**Status:** K3 closure complete; final ratchet has zero unexplained movement

**Base capability receipt:** K3l, commit `4c71a0959b8`

**Architectural authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

## Purpose

Finish K3's logical-flow and intrinsic-query work without turning the
architecture plan into a running task log. This document owns K3m onward:
slice order, live seams, acceptance evidence, stop rules, and per-slice
receipts. The architecture plan continues to own Buckram's model, later-stage
boundaries, cutover, and the final K3 closure receipt.

This is a serial execution plan for one implementation agent. Research may be
split out, but accepted implementation slices must land in the order below.
The core gates repeatedly touch the same box, block, adapter, and Livery
lowering seams, and they share one conformance ratchet.

## Scope correction

K3 owns:

- logical block and inline geometry;
- intrinsic inline and block queries needed by normal flow;
- BFC and IFC establishment and their baseline outputs;
- margin collapsing, floats, clearance, shrink-to-fit, and float avoidance;
- orthogonal-flow sizing needed to produce correct normal-flow fragments; and
- an explicit accounting of every remaining Taffy block dispatch.

The following work is routed forward:

- table-specific float avoidance and table wrapper behavior belong to K4;
- relative, absolute, fixed, and sticky positioning belong to K5;
- the out-of-flow IFC participant protected by K3b's cache guard belongs to
  K5 with static-position and containing-block work; and
- fragmentation-dependent sizing or continuation belongs to K6.

K3 may prepare inputs needed by those phases. It must not implement a partial
table or positioning model merely to remove a deferral.

## Starting receipt

K3l is the frozen starting point:

| Corpus | Pass | Fail | Skip | Error |
|---|---:|---:|---:|---:|
| `css/CSS2` | 4,233 | 1,741 | 3,279 | 1 |
| `css/css-backgrounds` | 241 | 348 | 360 | 0 |
| `css/css-borders` | 32 | 24 | 119 | 0 |
| `css/css-flexbox` | 384 | 502 | 472 | 0 |
| `css/css-grid` | 435 | 709 | 747 | 0 |
| `css/css-multicol` | 96 | 305 | 307 | 0 |
| `css/css-position` | 41 | 79 | 224 | 0 |
| `css/css-tables` | 50 | 80 | 198 | 0 |
| `css/css-writing-modes` | 226 | 887 | 255 | 0 |
| **All-nine pass total** | **5,738** | | | |

The only K3l status movement from K3j is
`css/CSS2/floats/floats-placement-005.html`, fail to pass. Exact expectations
and logs are under
`Code/testing/genet/wpt-ledger/2026-07-27_buckram_k3l`.

Counts are orientation, not acceptance. Every later comparison must list the
exact changed files and explain each movement.

Before K3m changes code, freeze a focused `css/css-sizing` expectation file at
the execution starting commit. K3a named that family, but the K3l all-nine
ledger does not contain it.

## Invariants

1. Buckram owns CSS boxes, formatting contexts, logical geometry, intrinsic
   queries, and fragments.
2. Taffy is an algorithm library for admitted flex and grid subtrees. A Taffy
   enum or node shape is not evidence of a CSS box role.
3. Generated-box roles decide admission. Backend display values do not.
4. Intrinsic size is a query over a box or subtree. It is not a style field or
   a value recovered from a completed backend layout.
5. A formatting context must expose the outputs its parent needs, including
   margins, intrinsic contributions, float continuation where applicable,
   baselines, overflow, and fragments.
6. A BFC isolates floats. An ordinary block in the same BFC may continue the
   parent's float state only through an explicit role.
7. Logical axes remain primary through layout. Physical geometry is derived at
   the fragment edge.
8. Existing deferrals are named capability boundaries. Remove or narrow one
   only when its replacement has pure, adapter, live, and corpus evidence.
9. `genet-layout` may be used as a differential oracle. Its implementation is
   not browser semantics.
10. Generated WPT expectations and build logs remain outside Git.

## Working-tree and commit discipline

- Start every slice with `git status --short` and record unrelated dirty
  paths. Preserve them.
- Stage only the files named by the accepted slice plus this plan's receipt.
- One gate produces one reviewable commit. Do not begin the next gate before
  the current gate passes its acceptance ladder.
- A failed broad admission experiment must be reverted or narrowed before the
  slice is committed. Record the useful boundary evidence in the receipt.
- Do not rewrite the K3l baseline. New expectation directories are descendants
  of that receipt.

## Live code map

| Seam | Current responsibility |
|---|---|
| `components/buckram/src/block.rs` | block equations, margin collapse, floats, clearance, BFC state, deferrals |
| `components/buckram/src/intrinsic.rs` | box-owned intrinsic query and cache contracts |
| `components/buckram/src/box_tree.rs` | outer and inner display roles, anonymous fixup, provenance, formatting-context roles |
| `components/buckram/src/fragment_tree.rs` | logical fragments, continuation fields, baselines, overflow |
| `components/buckram/src/taffy_adapter.rs` | Buckram block dispatch, explicit Taffy flex/grid calls, capability admission |
| `components/genet-livery/src/layout.rs` | computed-value lowering, generated-box role admission, live fragment construction |
| `components/genet-livery/src/text.rs` | IFC shaping, line breaking, inline fragments, baseline inputs |

The most relevant current boundaries are
`BlockDeferral::{ShrinkToFit, FloatShrinkToFit, FloatLineExclusion,
FloatFormattingContextAvoidance, NestedFloatState, IntrinsicSize,
IndependentFormattingContext, OrthogonalAutoBlockSize}`. K3 closure must also
inventory every other deferral and route it explicitly.

## Execution order

| Gate | Outcome | Depends on |
|---|---|---|
| K3m | general intrinsic widths and shrink-to-fit | K3l |
| K3n | automatic inline margins and non-table atomic BFC avoidance | K3m |
| K3o | durable generated-box provenance and `Block` context continuation | K3n |
| K3p | nested and inline float completion | K3o |
| K3q | intrinsic block queries and BFC baselines | K3p |
| K3r | orthogonal normal-flow finalisation | K3q |
| K3s | dispatch audit and K3 closure | K3r |

## K3m. General intrinsic widths and shrink-to-fit

### Problem

K3h admits only a static horizontal float whose auto width comes from exactly
one measured inline formatting context. Multi-child floats, block-content
floats, and inline-block shrink-to-fit still defer. Taffy's internal intrinsic
run modes cannot remain the browser-facing query API.

### Work

- Extend Buckram's intrinsic provider so a box can ask for the min-content and
  max-content inline contribution of an in-flow subtree.
- Define contribution rules for consecutive block children, inline formatting
  contexts, and admitted flex or grid children without flattening their CSS
  roles.
- Reuse `IntrinsicSizeCache` by `BoxId` and logical axis.
- Generalise `solve_float_shrink_to_fit_inline_size` to the proven multi-child
  and block-content shapes.
- Admit inline-block shrink-to-fit through the same query contract, while
  keeping float placement and ordinary atomic-inline placement distinct.
- Narrow `ShrinkToFit` and `FloatShrinkToFit` only for roles whose intrinsic
  providers are complete.

### Evidence

- Pure fixtures distinguish min-content and max-content for a subtree with at
  least two children.
- Adapter fixtures prove one multi-child float, one block-content float, and
  one inline-block at three available widths: below min-content, between the
  intrinsic bounds, and above max-content.
- A live Livery fixture proves the same border-box widths and zero accidental
  Taffy block fallback calls.
- Run focused `css/CSS2/floats`, `css/css-sizing`, and named inline-block
  families.

### Stop rules

- Stop if an intrinsic answer can be obtained only by performing final layout
  and reading a backend node.
- Stop if block children must be reclassified as inline leaves to share the
  existing measure callback.
- Keep a role deferred when percentage or cyclic intrinsic dependencies cannot
  be represented by the query contract.

### K3m receipt - 2026-07-28

Capability: Buckram now derives min-content and max-content inline sizes for
admitted in-flow subtrees, then uses the shared shrink-to-fit equation for
multi-child floats, block-content floats, and baseline-aligned atomic inline
blocks.

Boundary retained: descendant percentages, cyclic constraints, replaced boxes,
orthogonal flows, non-linear lengths, nested shrink-to-fit, descendant floats,
and non-baseline atomic inline alignment remain deferred. Flex and grid use
their intrinsic query mode only, never a recovered final layout.

Pure fixture: `auto_shrink_to_fit_width_clamps_available_space_between_intrinsic_sizes`.

Adapter fixture: `buckram_queries_multi_child_and_block_content_intrinsics_for_shrink_to_fit`
and `buckram_queries_atomic_inline_intrinsics_without_float_placement`, each
covering 30, 80, and 200 px available widths with zero Taffy block fallback.

Live fixture: `live_multi_child_float_and_atomic_inline_use_intrinsic_subtrees`
proves the two float shapes and 30, 80, and 200 px inline-block widths with
zero final Taffy block fallback.

WPT exact movement: `css/CSS2/floats` (48/53/43), `css/css-sizing`
(202/310/220), and `css/CSS2/normal-flow` (537/194/161) exactly match their
K3l or pre-change status maps. `inline-block-zorder-005.xht` passes on the
final runner.

Verification: `cargo test -p buckram -p livery -p genet-livery --offline`;
`cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings`;
`rustfmt --check`; `git diff --check`; release `genet-wpt` build.

Proof directory: `<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3m`

Commit: `Expand Buckram intrinsic shrink-to-fit queries`

## K3n. Automatic inline margins and non-table atomic BFC avoidance

### Problem

K3j resolves fixed BFC margins beside floats. Automatic inline margins and
some atomic formatting-context roots still bypass Buckram's float-avoidance
equation.

### Work

- Resolve automatic inline margins after the available float band is known.
- Preserve the CSS over-constraint rule and direction-sensitive ignored side.
- Admit non-table atomic-inline and block-level independent formatting
  contexts only when Buckram can supply their available inline size, recover
  their used block size, and place their fragments without consuming the
  baseline work reserved for K3q.
- Keep baseline-dependent atomic alignment deferred to K3q.
- Keep table wrappers routed to K4.
- Preserve K3k's rule that flex and grid parents remain on their established
  path when no active float requires Buckram placement.

### Evidence

- Pure equations cover one auto margin, two auto margins, over-constraint, and
  both inline directions.
- Adapter and live fixtures cover an atomic BFC that fits beside a float and
  one that moves below it.
- `css/CSS2/floats`, inline-block families, `css/css-flexbox`, and
  `css/css-grid` show zero unexplained movement.

### Stop rules

- Stop if an atomic child cannot provide its used block size without
  fabricating a leaf measure.
- Keep baseline-dependent atomic cases deferred until K3q rather than
  inventing an interim baseline.
- Route a table-shaped failure to K4 rather than admitting a grid surrogate.

### K3n receipt - 2026-07-28

Capability: automatic inline margins participate in Buckram's selected float
band. A non-table flow-root BFC fits beside a float when the band permits it,
or moves below and resolves its margins against the full containing block.

Boundary retained: baseline-dependent atomic inline alignment remains deferred
to K3q; table wrappers remain routed to K4; flex and grid retain their
established dispatch when no float is active.

Pure fixture: `block_width_equation_centres_two_auto_margins`,
`one_auto_inline_margin_uses_the_logical_start_in_both_directions`, and
`overconstrained_auto_inline_start_resolves_to_zero` cover two auto margins,
one auto margin, over-constraint, and both inline directions.

Adapter fixture: `buckram_remeasures_opted_in_bfcs_inside_the_float_band`
proves an admitted BFC that fits beside a float and a definite one that moves
below it without widening ordinary flex or grid dispatch.

Live fixture: `live_bfc_auto_margins_fit_or_move_below_floats_in_both_directions`
proves LTR and RTL flow-root placement, the below-float retry, and zero final
Taffy block fallback.

WPT exact movement: `css/CSS2/floats` (48/53/43), `css/CSS2/normal-flow`
(537/194/161), `css/css-flexbox` (383/503/472), and `css/css-grid`
(435/709/747) exactly match their K3m or K3l status maps.

Verification: `cargo test -p buckram -p livery -p genet-livery --offline`;
`cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings`;
`rustfmt --check`; `git diff --check`; release `genet-wpt` build.

Proof directory: `<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3n`

Commit: `Prove Buckram float-band auto margins`

## K3o. Durable generated-box provenance and `Block` context continuation

### Problem

K3l deliberately excludes generated `Block` formatting-context roots because
split inline continuations do not preserve enough provenance to distinguish a
block-level float from a float originating inside inline content. The current
live guard is safe but coarse.

### Work

- Add the smallest generated-box provenance needed to identify a float's
  originating formatting context after inline splitting and anonymous fixup.
- Keep outer display, inner display, formatting-context role, and provenance
  distinct.
- Replace ancestry heuristics in Livery admission with the generated-box fact.
- Admit ordinary generated `Block` formatting-context roots to shared float
  continuation only after split-inline floats remain distinguishable.
- Prove two or more nested ordinary wrappers, including collapsed margins and
  padding or borders that translate the content origin.

### Evidence

- Box-tree fixtures retain provenance through inline splitting and anonymous
  wrappers.
- Adapter fixtures translate exclusions through two ordinary blocks and export
  only newly created floats.
- Live fixtures cover a block-level nested float, a float originating inside
  inline content, and an explicit BFC boundary.
- Recheck the K3l diagnostic family:
  `floats-placement-005.html`,
  `floats-wrap-top-below-inline-003l.xht`, and
  `floats-placement-vertical-001b/001c.xht`.

### Stop rules

- Stop if provenance is inferred from Taffy display or tree position.
- Stop if one source element must be forced back into a single generated box.
- Keep the broad `Block` role deferred until the vertical split-inline
  counterexamples retain their old status or improve for a separately proven
  reason.

### Receipt

Capability: Generated boxes now carry explicit `Block` or `Inline` float-origin
provenance through blockification, split-inline continuation, and anonymous
fixup. Ordinary generated `Block` formatting-context roots can continue a
parent float context when their existing role checks pass.

Boundary: Inline-origin floats remain explicitly marked for the adapter's
deferred lane. This does not admit nested-inline float layout, signed-margin
float geometry, nowrap line decisions, or the broad `Block` role.

Pure: `float_context_provenance_survives_inline_splitting_and_anonymous_fixup`
proves both split continuations receive anonymous wrappers while their direct
inline float retains `Inline` provenance and a nested block float retains
`Block` provenance.

Adapter: `buckram_exports_nested_floats_through_two_ordinary_blocks_to_outer_siblings`
proves newly created float state crosses two ordinary blocks to a following
clear; the existing inline-origin and explicit-BFC fixtures remain green.

Live: `live_generated_block_roots_translate_nested_float_state` proves a
collapsed 10px/20px margin chain plus 3px border and 5px padding translates a
nested float to y=28px and its following clear to y=68px. Livery's DOM/style
box-tree fixture retains `Inline` provenance for a floated inline continuation.

WPT: The K3l diagnostic files `floats-placement-005.html`,
`floats-wrap-top-below-inline-003l.xht`, and
`floats-placement-vertical-001b/001c.xht` each retain a one-file `run`
crash-smoke result of 1 passed, 0 failed, 0 errored. This is not a
behavioral testharness or reftest claim.

Verification: `cargo test -p buckram -p livery -p genet-livery --offline`,
`cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings`,
edition-2024 `rustfmt --check` on touched Rust files, `git diff --check`, and
`cargo build -p genet-wpt --release --all-features --offline` all passed from
an isolated K3o target.

Proof directory: `<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3o`

Commit: `Preserve Buckram float-context provenance`

## K3p. Nested and inline float completion

### Problem

Negative-margin floats, nested inline contexts, nowrap content, and some
per-line exclusions remain outside K3l's shared state. These combine signed
margin-box geometry with IFC line decisions.

### Work

- Represent signed float margin-box extents without treating a negative used
  block size as a valid exclusion.
- Generalise the translated-state fixed point for negative margins and
  collapsing descendant origins.
- Deliver float bands through nested inline formatting contexts.
- Define nowrap behavior as one line constrained by the relevant float band,
  not as a max-content shortcut.
- Keep bidi ordering, baseline alignment, and line fragmentation in the IFC.
- Remove nested-clear and split-inline guards only for the exact proven roles.

### Evidence

- Pure fixtures cover signed margins on both float sides and translated
  exclusions that begin above the descendant content origin.
- Adapter fixtures cover convergence and a named non-convergence deferral.
- Live fixtures cover nested inline content, nowrap, bidi direction, and the
  `margin-collapse-135.xht` counterexample that protected K3l.
- Run focused CSS2 float, float-clear, text, and margin-collapse families.

### Stop rules

- Stop if the fixed point has no bounded monotone progress measure.
- Stop if nowrap is implemented by bypassing IFC line construction.
- Preserve the old path when a nested inline box lacks a stable fragment or
  baseline output.

### K3p receipt (2026-07-28)

`FloatMarginBox` now carries a `SignedBlockExtent`: a float with a negative
used block-size still positions its border box, but is inert for exclusion,
clearance, line constraints, and BFC height. Positive exclusions translated
above a descendant content origin remain active. The ordinary-block
translated-state loop now admits negative margins; a converging nested-float
fixture reaches the outer clearer, while an oscillating origin fixture exhausts
its bounded retries and deliberately returns to Taffy.

All admitted inline groups receive float constraints. The IFC keeps `nowrap`
as a single unbounded Parley line, but selects its line origin and any wider
float band through the same breaker used for wrapped lines. The live receipt
covers nested inline content in LTR and RTL. Nested clear without a shared
role and floats originating inside an inline context remain deferred because
their fragment and baseline paths are still not stable.

Verification: `cargo test -p buckram -p livery -p genet-livery --offline`,
the post-suite `nested_float_state_nonconvergence_remains_deferred` fixture,
`cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings`,
edition-2024 `rustfmt --check` on touched Rust files, and `git diff --check`
passed. `cargo build -p genet-wpt --release --all-features --offline` passed
from a temporary isolated target with `RUSTUP_TOOLCHAIN=1.97.1-x86_64-pc-windows-msvc`;
the override prevents the `oxc-miette` dependency's local 1.94 toolchain file
from mixing compilers in the same target.

WPT crash-smoke: `negative-margin-float-positioning.html`,
`negative-block-margin-pushing-float-out-of-block-formatting-context.html`,
`float-nowrap-5.html`,
`float-in-inline-anonymous-block-with-overflow-hidden.html`,
`margin-collapse-135.xht`, and `text/bidi-span-001.html` each report 1
passed, 0 failed, 0 errored. These are parser/layout crash-smoke results, not
behavioral reftest or testharness claims.

Proof directory: `<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3p`

Commit: `Complete Buckram nested float state and nowrap bands`

## K3q. Intrinsic block queries and independent-BFC baselines

### Problem

The cache contract supports both logical axes, but the live provider currently
answers retained inline widths. Independent formatting contexts also need
baseline outputs that the parent can consume without inspecting backend
children.

### Work

- Define min-content and max-content block-axis queries only where CSS gives
  them a stable meaning without fragmentation.
- Make query cycles and indefinite containing sizes explicit outcomes.
- Populate Buckram fragment baselines for IFC, block, flex, grid, and admitted
  atomic formatting contexts.
- Return first and last baselines from an independent BFC as modeled outputs.
- Keep fragmentation-dependent block contributions routed to K6.

### Evidence

- Pure intrinsic fixtures distinguish inline and block axes and exercise a
  cycle or indefinite-basis result.
- Fragment-tree fixtures assert first and last baselines independently of the
  physical rectangle.
- Adapter and live fixtures align an independent BFC with text and with an
  admitted flex or grid child.
- Run named `css-align`, `css-flexbox`, `css-grid`, `css-sizing`, and CSS2
  vertical-align families.

### Stop rules

- Stop if a block intrinsic query silently substitutes `auto`, zero, or the
  completed used size.
- Stop if a baseline is recovered by walking Taffy descendants after the
  formatting context returns.
- Route fragmentainer-dependent answers to K6.

### K3q receipt - 2026-07-28

Buckram now exposes an explicit unfragmented block-axis intrinsic query seam.
It requires a finite, definite inline basis and returns one measured block
contribution for both min-content and max-content. Unsupported axes,
indefinite bases, query cycles, and fragmentainer-dependent contributions are
explicit results; failed queries do not enter the cache. Fragmentainer work
remains K6. Livery's retained intrinsic provider continues to answer inline
widths, so CSS property admission that needs a fragmentainer-aware block
contribution has not been implied here.

`Baselines` are finite logical offsets from a fragment's block-start edge.
`AlgorithmTree` retains each formatting context's declared output, synthesizes
the block-end fallback where there is no line baseline, and lets parents consume
that output with the copied child placement. It does not recover a baseline by
walking Taffy descendants after layout. Retained IFCs supply their first and
last shaped-text baselines; block, flex, grid, and admitted atomic contexts
expose a modeled result. The currently admitted atomic, empty flex, and empty
grid cases use their own block-end fallback. Livery copies those outputs into
its fragment tree unchanged.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: pass.
- `cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings`:
  pass.
- `rustfmt --check` on the six changed Rust files and `git diff --check`:
  pass.
- Release `genet-wpt --all-features --offline` build: pass under Rust 1.97.
- Named WPT crash-smoke, not behavioral reftest or testharness evidence:
  `css-align` 293/297, `css-flexbox` 1321/1358, `css-grid` 1888/1891,
  `css-sizing` 732/732, and CSS2 vertical-align's `linebox` group 200/249;
  each completed with zero failed and zero errored files.

Proof directory:
`<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3q`

Commit: `Complete Buckram intrinsic block queries and BFC baselines`

## K3r. Orthogonal normal-flow finalisation

### Problem

`OrthogonalAutoBlockSize` still protects Buckram from converting an
auto-sized vertical or sideways block into physical geometry too early.
Shared float continuation also requires an explicit policy when child and
containing flows differ.

### Work

- Keep logical containing sizes and available space intact until block layout
  completes.
- Finalise auto block size in the box's own flow, then derive physical size and
  position at the fragment edge.
- Define the containing-block transformation for orthogonal children.
- Use the intrinsic and baseline outputs from K3q rather than querying backend
  physical dimensions.
- Admit cross-flow float continuation only where CSS says the boxes share one
  BFC and the exclusion can be transformed without losing its logical sides.
- Narrow or remove `OrthogonalAutoBlockSize` for the proven normal-flow roles.

### Evidence

- Pure flow fixtures cover vertical-rl, vertical-lr, sideways-rl, and
  sideways-lr with auto block size.
- Adapter fixtures nest horizontal and vertical flows in both directions.
- Live fragments prove logical size, physical rectangle, baseline, and
  containing fragment.
- Run complete `css/css-writing-modes`, focused orthogonal sizing families,
  and complete CSS2.

### Stop rules

- Stop if physical width or height becomes the primary value inside the block
  algorithm.
- Stop if float sides are copied across flows without an explicit logical
  transform.
- Route multi-fragment vertical sizing to K6.

### K3r receipt - 2026-07-28

Buckram now keeps normal-flow blocks in their own logical axes while their
auto block size is unresolved. Specified physical CSS width and height are
resolved against the containing block, then converted to the box's own axes.
Child placement is retained as a logical rectangle until the parent's block
contribution is complete, when the final physical outer size and child
locations are derived at the fragment edge. This closes normal flow for
vertical-rl, vertical-lr, sideways-rl, sideways-lr, and horizontal/vertical
parent-child nesting. `OrthogonalAutoBlockSize` is removed.

Livery now retains the same two-coordinate contract for non-horizontal
fragments: absolute physical rectangles for paint plus flow-relative logical
rectangles for layout consumers. The retained IFC collector follows that
conversion too. The live vertical-rl proof covers generated boxes, logical and
physical fragment geometry, first and last baselines, and a containing
fragment.

Orthogonal float and clearance continuation remains deferred. Buckram does
not copy physical `left` or `right` state across a horizontal/vertical
boundary without an explicit logical transform. Multi-fragment vertical sizing
remains K6.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: pass, 27 test
  targets reported success.
- `cargo clippy -p buckram --offline --no-deps -- -D warnings`: pass. The
  combined Buckram and Genet-Livery command is currently blocked by
  `clippy::implicit_saturating_sub` in `components/genet-livery/src/text.rs`,
  introduced by committed selection work outside K3r; this slice does not
  alter that file.
- `rustfmt --check` on the changed Rust files and `git diff --check`: pass.
- Release `genet-wpt --all-features --offline` build: pass under Rust 1.97.
- Named WPT crash-smoke, not behavioral reftest or testharness evidence:
  `css/css-writing-modes` 1233/1368, `css/css-sizing` 732/732, and `css/CSS2`
  6383/9254; each completed with zero failed and zero errored files.

Proof directory:
`<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3r`

Commit: `Complete Buckram orthogonal normal flow`

## K3s. Dispatch audit and closure

### Work

- Inventory every `BlockDeferral` variant and every remaining Taffy block
  dispatch.
- For each survivor, record one of:
  - closed by K3 with structural and corpus evidence;
  - owned by K4, K5, or K6;
  - a named capability gap outside the selected cutover corpus; or
  - an unexplained blocker, which prevents K3 closure.
- Delete obsolete admission flags, diagnostic switches, and compatibility
  paths made unreachable by K3m through K3r.
- Run the complete acceptance ladder.
- Append the final K3 closure receipt to the architecture plan and replace its
  open paragraph with the routed K4, K5, and K6 work.

### Closure evidence

- Every formatting-context role used by the selected corpus has an owned box,
  intrinsic, baseline, and fragment contract.
- Every retained Taffy call receives CSS inputs from Buckram and returns a
  modeled formatting-context output.
- Focused structural fixtures distinguish min-content from max-content on both
  supported logical axes.
- Complete CSS2, `css/css-sizing`, `css/css-writing-modes`, and the all-nine
  ratchet have zero unexplained regressions against their frozen predecessors.
- The final receipt states exact changed files, not only aggregate counts.

### K3s receipt - 2026-07-28

Capability: the K3 dispatch audit is complete. Buckram records the
first CSS-facing `BlockDeferral` on every Taffy block fallback, while an
already-active fallback's descendants record the adapter-only
`BackendSizingMode`. A dynamic child fallback now preserves its original
reason through its parent. `ParentMarginCollapse` is deleted because an
admitted Buckram block child must return modeled margin output.

Boundary retained: the architecture plan's K3 dispatch audit inventories the
surviving deferrals and routes table behavior to K4, positioning and the
out-of-flow IFC participant to K5, fragmentation-dependent answers to K6, and
the remaining unsupported intrinsic, replaced, containment, nonlinear, and
orthogonal-float shapes as named post-cutover gaps.

Pure fixture: `taffy_block_fallback_retains_the_css_facing_deferral` proves a
replaced root preserves `Replaced` while its nested Taffy block records only
`BackendSizingMode`. `nested_float_state_nonconvergence_remains_deferred`
proves a dynamic child fallback reaches the root as `NestedFloatState`.

Adapter fixture: `orthogonal_float_continuation_stays_deferred_without_a_logical_transform`
records `NestedFloatState` rather than copying physical float sides across
flows. The K3m through K3r structural fixtures remain the owned normal-flow,
intrinsic, BFC, IFC, baseline, and logical-fragment proofs.

Live fixture: `ordinary_live_block_flow_uses_buckram_without_backend_dispatch`,
`live_nested_float_state_crosses_ordinary_wrappers_but_stops_at_bfcs`, and
`live_orthogonal_normal_flow_preserves_logical_fragment_geometry_and_baseline`
cover the selected Livery path with generated boxes, fragments, float
boundaries, baselines, and zero accidental Taffy block fallback.

WPT exact movement: all nine fresh Livery reftest maps retain their K3l file
cardinality, 16,375 URLs total. They move 69 fail-to-pass and 45 pass-to-fail
statuses: 5,738 to 5,762 passes. The full per-file delta is
`all-nine-delta-details.txt` in the proof directory. The 45 regressions block
the final K3 closure receipt; they are not relabelled as accepted forward work.

Verification:

- `cargo test -p buckram -p livery -p genet-livery --offline`: pass, 27 test
  targets reported success on the final audited source.
- `cargo clippy -p buckram --offline --no-deps -- -D warnings`: pass. The
  combined Buckram and Genet-Livery command remains blocked by the unrelated
  `clippy::implicit_saturating_sub` warning in `components/genet-livery/src/text.rs`.
- Release `genet-wpt --all-features --offline` build: pass under Rust 1.97.
- Fresh Livery reftest maps ran all nine K3 directories; their nonzero runner
  exits reflect expected failing reftests, while the written JSON maps provide
  the exact status comparison above.

Proof directory:
`<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3s`

K3 closure does not mean complete CSS layout. It means the remaining work is
truthfully owned by K4, K5, K6, or an explicitly named post-cutover gap.

### K3t closure receipt - 2026-07-28

Capability: the final correction keeps orthogonal normal-flow children
indefinite in the parent’s cross-flow axis, lowers `inline-size` after the
winning writing mode is known, and gives retained text a real forced-break
line with its own line-height. It also removes the duplicate negative inline
margin shift from glyph paint. The resulting live path fixes the K3r
orthogonal percentage cases, vertical inline decoration, body/root writing
mode sizing, and the CSS2 replacement and preserved-newline references that
had been passing accidentally.

Boundary retained: K3 does not claim the newly exposed non-normal-flow
families. The 21 K3s-to-K3t pass-to-fail movements are all exact false-pass
disclosures, not newly broken behavior:

- `css/CSS2/floats/float-no-content-beside-001.html` and
  `css/CSS2/linebox/line-breaking-font-size-zero-001.html` need retained IFC
  empty-line and zero-font break opportunities integrated with float
  constraints.
- Twelve `css/css-grid/abspos/grid-abspos-staticpos-*` files and
  `css/css-writing-modes/abs-pos-border-offset-{001,002}.html` belong to K5’s
  static-position and positioned containing-block work.
- `css/css-grid/alignment/grid-baseline-align-cycles-001.html` and
  `css/css-grid/firefox-bug-1881495.html` need Taffy-grid cyclic baseline and
  intrinsic auto-block-size behavior beyond K3’s admitted BFC baseline
  output.
- `css/css-writing-modes/full-width-{002,003}.html` require
  `text-combine-upright`; `wm-propagation-body-054.html` needs principal-flow
  propagation through generated pseudo content and upright text orientation.

WPT exact movement: against K3s, the nine fresh Livery maps move 64 failures
to passes and the 21 named false passes to failures, from 5,762 to 5,805
passes. The per-corpus counts and every changed URL are in
`final-all-nine-delta-summary.txt` and `final-all-nine-delta-details.txt` in
the proof directory below. The ratchet therefore has zero unexplained
regressions.

Verification: `cargo test -p buckram -p livery -p genet-livery --offline`;
`cargo clippy -p buckram --offline --no-deps -- -D warnings`; edition-2024
Rustfmt on touched files; `git diff --check`; and a release `genet-wpt` build
all pass. The all-nine runner’s nonzero per-corpus exits represent expected
failing reftests; each wrote its status map.

Proof directory:
`<workspace>\testing\genet\wpt-ledger\2026-07-28_buckram_k3t`

Commit: `Complete Buckram K3 closure ratchet`

## Acceptance ladder for every gate

1. **Model proof:** pure Buckram fixture names the CSS distinction.
2. **Adapter proof:** Buckram owns the block decision and Taffy is called only
   for an admitted algorithm subtree.
3. **Live proof:** generated boxes, computed values, fragments, and algorithm
   counters show the same behavior through Livery.
4. **Focused corpus:** fresh expectations for the named WPT family.
5. **Regression ratchet:** exact status comparison against the prior accepted
   gate, including complete CSS2 whenever CSS2 moves.
6. **Build proof:**

   ```powershell
   $env:CARGO_TARGET_DIR = 'C:\t\graphshell-target'
   cargo test -p buckram -p livery -p genet-livery --offline
   cargo clippy -p buckram -p genet-livery --offline --no-deps -- -D warnings
   rustfmt --edition 2024 --check `
     components/buckram/src/block.rs `
     components/buckram/src/box_tree.rs `
     components/buckram/src/fragment_tree.rs `
     components/buckram/src/intrinsic.rs `
     components/buckram/src/taffy_adapter.rs `
     components/genet-livery/src/layout.rs `
     components/genet-livery/src/text.rs
   git diff --check
   cargo build -p genet-wpt --release --all-features --offline
   ```

Run Rustfmt only on touched Rust files. Missing paths from a slice are omitted
from that command.

When a focused corpus moves, run the complete owning directory fresh. When any
all-nine directory moves, compare all nine exact expectation maps before
acceptance.

## Receipt template

Append one receipt beneath the completed gate in this document:

```markdown
### K3x receipt - YYYY-MM-DD

Capability:

Boundary retained:

Pure fixture:

Adapter fixture:

Live fixture:

WPT exact movement:

Verification:

Proof directory:

Commit:
```

Record failed broad experiments when they identify a durable boundary. Do not
retain temporary diagnostic switches in accepted source.

## Subagent handoff

The implementation handoff is:

> Read this document, the architecture plan through K3l, and the live seams
> named under K3m. Execute K3m only. Preserve unrelated worktree changes.
> Begin with a pre-change `css/css-sizing` baseline. Stop after K3m passes the
> full acceptance ladder, append its receipt here, stage only K3m paths, and
> commit. Do not begin K3n in the same task.

After review, the next task receives the next gate and its predecessor's
accepted receipt. This keeps the conformance baseline and architectural
boundary observable at every handoff.
