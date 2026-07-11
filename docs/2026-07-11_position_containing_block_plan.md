# Position containing blocks: fixed to the ICB, absolute to its positioned ancestor

**Date:** 2026-07-11
**Status:** **F1 and F2 both landed 2026-07-11** (fixed -> ICB; absolute ->
nearest positioned ancestor; results and residuals below). Remaining work is
the F3 out-of-scope list plus the named residuals. Spun out of the layout
roadmap's near-horizon entry (found 2026-07-10 by the WPT input-path work, H9
in the harness-exactness plan).
**Scope:** the *layout* half of out-of-flow positioning in `serval-layout`. The
paint half (deferred stacking layers, fixed layers countering document scroll)
already exists (`is_out_of_flow` / `is_fixed` feeding `paint_stacking`, per the
zindex scope doc's rule 3) and is not reopened here.

## The gap, precisely

serval-layout has no containing-block concept. Every box's Taffy node parents
under its DOM parent's node, and Taffy's absolute positioning is
parent-relative by construction, so `position: fixed` and `position: absolute`
both resolve insets and percentages against the **Taffy parent** instead of the
CSS containing block. Diagnosed by probe: a
`{ position: fixed; top:0; right:0; bottom:0; left:0 }` div under a normal
`<body>` computes to `(0, 0, 800, 0)` — width resolves (from left+right), height
is 0 because `body`'s auto height is 0 (its only child is out of flow, and Taffy
correctly excludes abspos children from content sizing). Give `body` a height
and the same box resolves fully. So **inset-derived sizing already works; only
the parent is wrong.** The whole fix is making the Taffy parent *be* the CSS
containing block.

## The standard

- **CSS 2.2 §10.1 / css-position-3 §2.1** — the containing block per position
  value: `absolute` → the **padding box** of the nearest ancestor with
  `position ≠ static`, else the ICB; `fixed` → the viewport (the ICB).
- **css-transforms-1 §2** (and filter-effects, css-contain) — an ancestor with
  `transform ≠ none` (or `will-change: transform`, `filter`, `perspective`,
  `contain: paint/layout`) **becomes the containing block for all descendants,
  including fixed**. Load-bearing here twice over: it is the spec rule *and* it
  is what keeps the orrery correct (camera-transformed `.stage` with abs-pos
  gnodes must not be hoisted out — and per this rule, must not be).
- **CSS 2.2 §10.3.7 / §10.6.4** — static position (auto insets) may be
  approximated: "user agents are free to make a guess at its probable
  position." A first cut leans on this; real `fixed` usage nearly always sets
  insets.

Reference implementation for shape (donor rule: cite, don't copy): Servo's
`PositioningContext` + `HoistedAbsolutelyPositionedBox` — out-of-flow boxes
register with a context during construction and lay out once their containing
block has a final size. serval's Taffy translation is cheaper: reparent the
box's Taffy node so ordinary parent-relative resolution does the rest.

## Mechanism (grounded in the actual construction walk)

`build_node` (`box_tree.rs`) is the single recursive choke point: every element
box — block child, table cell, multi-root child, replaced leaf, inline-context
leaf — is created inside it and attached to its parent by arena index. So:

1. **A transform-CB depth counter on the under-construction tree.**
   `build_node` computes `establishes_fixed_cb(&style)` at entry (as landed:
   `transform` / `perspective` / `filter`; `will-change` and `contain` are a
   named residual), increments the counter around its child recursion, and
   snapshots the depth at entry for its own hoist decision (a box's *own*
   transform does not change its own containing block — only ancestors count).
2. **Hoist registration, not in-place reparenting.** When the finished box is
   `position: fixed` and the entry depth was zero, `build_node` records its
   arena index in `tree.fixed_hoists`. The parent still receives the index
   (zero signature churn across the five call sites).
3. **One post-pass in `build_box_tree`,** after the root exists: strip the
   hoisted indices from every `children` list, append them to the root's.
   Document order among hoisted boxes is preserved (registration order).
   Cascade and inheritance are untouched — stylo already ran over the DOM.

The ICB approximation: the root box. For parsed HTML that is `<html>`, which
the UA sheet sizes to the viewport (`width/height: 100%`), so insets resolve
against 800x600 exactly. For synthetic multi-root documents the root is
content-sized; accepted for F1 and noted below.

## Phases

### F1 — `fixed` → ICB *(this slice)*

The mechanism above, `fixed` only. `absolute` behavior is untouched, which
also means the orrery is untouched twice over (gnodes are `absolute`, and
their `.stage` is transformed).

**Done when:**

- The probe becomes a guard: the WPT-shaped fixed div computes `(0,0,800,600)`
  under an auto-height body, and hit-tests at the viewport center.
- A fixed box under a **transformed** ancestor is *not* hoisted (the
  css-transforms rule, and the safety rail).
- A `static → fixed` toggle through `IncrementalLayout::apply` re-resolves
  (exercises the incremental path; a position flip must reach a rebuild).
- WPT: the four `wheel`/`mousewheel`-on-`div` tests in
  `dom/events/non-cancelable-when-passive` go green (cluster 42/42); every
  other checked baseline stays `unexpected=0` or is deliberately rebased with
  the delta named; meerkat/orrery suites stay green (mere consumes local
  serval — meerkat chrome is real exposure).

**Landed 2026-07-11, all conditions met.**
`dom/events/non-cancelable-when-passive` is **42/42 all-pass (53/53 subtests)**;
`dom` overall gained +4 (`fail -> pass`, the wheel quartet), rebased to
148 all-pass with `unexpected=0`; all other baselines unchanged; serval-layout
298, xilem-serval 101, serval-scripted 79 (both engines), meerkat 247, all
green. Guards: `fixed_inset_box_resolves_against_the_viewport`,
`a_fixed_box_under_a_transformed_ancestor_is_not_hoisted`,
`toggling_position_to_fixed_rehosts_to_the_viewport`.

**What the slice actually took, beyond the box-tree hoist** (the plan's
"paint/hit parity" hazard was real, in the *hit* lane):

- The hit walk (`serval_lane::walk_for_hit`) is **DOM-driven**, while the hoist
  is a *box-tree* fact, and this diverged twice:
  1. **The overflow clip-prune swallowed escaped fixed boxes.** `body` at
     `overflow: hidden` with auto height 0 pruned its whole DOM subtree from
     hit-testing — including the fixed child whose containing block (the
     viewport) the clipper is not in. Per CSS Overflow, clipping applies only
     through the containing-block chain. The prune now defers direct
     escaped-fixed children before returning.
  2. **Origin accumulation double-counted.** A hoisted box's fragment location
     is root-relative; summing it along the DOM chain adds the ancestors'
     offsets again (on a default page, exactly the 8px `<body>` margin — hit
     and paint would disagree by it). The walk now computes an **escaped**
     fixed box's origin standalone. Escape state threads through the walk and
     the deferred-subtree queue as `ancestor_fixed_cb`, derived from the same
     `establishes_fixed_cb` predicate the box tree uses, so the two stay in
     agreement by construction.

**F1 residuals, named:**

- `absolute_origin` / `walk_origins` (backing `absolute_rect`, host overlays,
  a11y bounds) still accumulate the DOM chain, so a hoisted fixed box's
  reported rect is off by its ancestors' offsets on pages with body
  margin/padding. Same fix shape as the hit walk; it is style-blind today
  (needs a `StylePlane` parameter), hence deferred rather than folded in.
- Escaped-fixed boxes nested *deeper* inside a clip-pruned subtree are still
  missed by hit-testing (the prune defers direct children only).
- An escaped fixed box under an element-scrolled (but untransformed) ancestor
  inherits that ancestor's scroll mapping in the hit point; per spec it should
  not. Edge; noted in the walk comment.
- `establishes_fixed_cb` consults `transform` / `perspective` / `filter`;
  `will-change` and `contain` are not yet read (a box a real UA would keep
  local may hoist).

### F2 — `absolute` → nearest positioned ancestor

Second anchor on the same walk (pushed on `position ≠ static` and the
transform-CB set), hoisting `absolute` boxes to it. Bites only when static
wrappers sit between the box and its positioned ancestor (the common
abspos-directly-inside-relative-parent pattern is already correct today).
Verify at implementation: the CB is the ancestor's **padding box**; check which
box Taffy resolves abspos children against and shim the offset if needed.
Static-position fidelity (auto insets) also lands here, approximated per the
spec's latitude. Blast-radius guard: the CSS corpora plus orrery/meerkat.

**Landed 2026-07-11.** The padding-box question was answered by probe before
committing to a design: Taffy already resolves an abspos child's insets against
the parent's **padding box** (border-box origin + border, spec-correct), so no
offset shim was needed — reparenting alone is the whole fix. Mechanism as
planned, with these concretions:

- `tree.abs_cb: Option<Id>` (the nearest absolute-CB ancestor's DOM id,
  save/restore around the child recursion) alongside F1's depth counter. The
  hoist registers `(arena index, cb)` — `None` meaning the ICB — and only when
  the CB differs from the DOM parent (a CB that *is* the parent is already
  what Taffy resolves against; reparenting would be a no-op churning child
  order).
- **Auto insets refuse the hoist.** A box with all four insets `auto` sits at
  its static position (§10.3.7), which its DOM parent approximates far better
  than the distant CB would, and there are no insets to resolve anyway.
- The post-pass resolves the CB's DOM id through `node_map`; a CB whose box is
  a leaf that lays out no children (replaced, inline-formatting,
  external-texture, chisel) refuses the hoist and the box stays
  parent-relative, the pre-F2 approximation.
- The graft guard and the fragment-plane hoist side table generalize over both
  hoist lanes (`had_hoists`; the readback DFS covers `fixed_hoists` and
  `abs_hoists`), so hit-testing, `absolute_rect`, and the clip-prune deferral
  cover hoisted absolute boxes with no walker changes at all.

Guards: `an_absolute_box_skips_static_wrappers_to_its_positioned_ancestor`
(padding-box discriminator + hit test),
`an_absolute_box_with_all_auto_insets_stays_at_its_static_position`,
`an_absolute_box_with_no_positioned_ancestor_resolves_against_the_icb` (the
behavior change), `percentage_geometry_resolves_against_the_positioned_ancestor`,
`toggling_position_to_absolute_rehosts_to_the_positioned_ancestor` (the
incremental-splice hazard, F2 lane).

Results: serval-layout 304, paint html→pixels 30, xilem-serval 101,
serval-scripted 45, meerkat 247, all green. WPT: all seven baselines hold at
`unexpected=0` except `fetch_api_basic`, whose 2–4 fluctuating deltas
reproduce **without** F2 (control rebuild) — environmental, left unrebased.
`css/css-position` reftests **60 → 62** passed (control-measured), testharness
lane unchanged (22 all-pass — those tests probe computed values, not
geometry); both lanes now have seeded baselines
(`expectations/{testharness,reftest}/css_position_boa.json`) for governance.

**F2 residuals, named:**

- **Partial-auto insets hoist with an approximated static side.** A box with
  e.g. only `left` set hoists; Taffy then derives the auto `top` from its
  static position *within the CB's flow*, not within the original DOM
  parent's flow (spec wants the latter). Same §10.3.7 latitude as the
  all-auto case, but the guess is coarser once hoisted.
- All-auto-inset boxes stay fully parent-relative (deliberate; see above) —
  their *sizing* CB (for percentage widths) is therefore still the DOM parent,
  not the positioned ancestor.
- The F1 hit-walk residuals (deeper-nested hoisted boxes under a clip-pruned
  subtree; element-scrolled ancestor point mapping) apply to hoisted absolute
  boxes equally.

### F3 — out of scope

`sticky` (scroll-linked, different machinery), inline-level fixed/absolute
blockification (an out-of-flow inline should blockify; today a fixed `<span>`
rides inline content and never reaches `build_node`'s hoist), and synthetic
multi-root ICB sizing. Each is noted so it is not lost, none blocks F1/F2.

## Hazards, named

- **Incremental splice.** A hoisted box's Taffy node lives under the root, not
  its DOM subtree. Structural mutations that rebuild the box tree are safe (the
  post-pass reruns); the hazard is any splice path that grafts a DOM subtree
  containing a fixed box without a rebuild. The toggle test probes the position
  flip; if a splice path survives with stale parentage, fall back to full
  rebuild on subtrees containing fixed boxes.
- **Paint/hit parity.** `paint_stacking` already lifts out-of-flow boxes into
  deferred layers, so tree order matters less than it looks, but the suites are
  the check, not the argument.
- **Consumer exposure.** meerkat's chrome sheets may use `position: fixed`;
  mere builds against local serval. Run its suite before calling the slice
  done.
