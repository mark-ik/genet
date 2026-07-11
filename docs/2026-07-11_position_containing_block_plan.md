# Position containing blocks: fixed to the ICB, absolute to its positioned ancestor

**Date:** 2026-07-11
**Status:** plan + F1 slice in progress. Spun out of the layout roadmap's
near-horizon entry (found 2026-07-10 by the WPT input-path work, H9 in the
harness-exactness plan).
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
   `build_node` computes `establishes_fixed_cb(&style)` at entry
   (transform / will-change:transform / filter / perspective /
   contain:paint|layout), increments the counter around its child recursion,
   and snapshots the depth at entry for its own hoist decision (a box's *own*
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

### F2 — `absolute` → nearest positioned ancestor

Second anchor on the same walk (pushed on `position ≠ static` and the
transform-CB set), hoisting `absolute` boxes to it. Bites only when static
wrappers sit between the box and its positioned ancestor (the common
abspos-directly-inside-relative-parent pattern is already correct today).
Verify at implementation: the CB is the ancestor's **padding box**; check which
box Taffy resolves abspos children against and shim the offset if needed.
Static-position fidelity (auto insets) also lands here, approximated per the
spec's latitude. Blast-radius guard: the CSS corpora plus orrery/meerkat.

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
