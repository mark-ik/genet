# Buckram K3 completion execution plan

**Status:** ready for serial execution

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

K3 closure does not mean complete CSS layout. It means the remaining work is
truthfully owned by K4, K5, K6, or an explicitly named post-cutover gap.

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
