# serval-layout: infrastructure scope (post emit-feature push)

**Status (2026-06-07, refreshed 2026-06-09):** scoping, for review. The
reftest scoreboard has not been updated here; record the next `serval-wpt`
measurement in the conformance plan once it lands.

The recent serval-layout work (gradients incl. repeating-linear, the
text-decoration trio, letter/word-spacing, the cascade-origin fix, and the full
list-marker feature) drained the clean *emit-side* backlog. Every remaining item
in the deferred inventory needs one of four pieces of infrastructure, scoped
below. Each section is grounded against the current code, with the blocker, the
approach, the touch-points, and a done-condition.

Recommended order is by value over effort: **interaction state** (mostly wired,
high value), **font system** (self-contained), **quirks mode** (small), then
**pseudo-element cascade** (largest, but unblocks the most).

---

## 1. Interaction state (`:hover` / `:focus` / selection / focus queries)

**Unblocks:** dynamic pseudo-classes (`:hover`, `:focus`, `:active`,
`:focus-within`, `:checked`, …) actually changing style; full
`ServalLaneView` interaction state read-back; selection-range geometry.

**Current state (already built):** `StyleEntry` carries a `state: ElementState`
field, and `adapter_stylo`'s `match_non_ts_pseudo_class` already matches the
state-backed pseudo-classes against it (`adapter_stylo.rs` ~563, `state()` at
~727). So the *matching* half is done. `ServalLaneView` also now has partial
read-back: `affordances_at` / `activation_target` derive from the DOM at the hit
point, and `focus_target` / `selection` return host-supplied state from
`with_interaction`. What is still missing is the *cascade source* of interaction
state: populating `StyleEntry::state` from host hover/focus/active state and
running the corresponding restyle/invalidation path.

**Approach:**
- A host-owned `InteractionState` (focused node, hover/active node chain,
  selection range), passed into the restyle path. Before a restyle, populate
  each affected `StyleEntry::state` from it (set/clear `HOVER` / `FOCUS` /
  `ACTIVE` / `FOCUS_WITHIN` on the right nodes), then let Stylo's existing
  state-change invalidation restyle the minimum.
- Extend the current `ServalLaneView::with_interaction` path, or replace it with
  the shared `InteractionState`, so read-back and cascade consume the same
  snapshot. `activation_target` already walks the hit node up to the nearest
  activatable ancestor; richer inline hit-testing can refine it later.
- Selection-range geometry (`rects_for_selection`) is a separate, larger sub-task
  (needs line-box rects threaded into the `FragmentPlane`); split it out.

**Touch-points:** `cascade.rs` (state population + invalidation), `serval_lane.rs`
(shared snapshot plumbing, not basic query implementation), a new
`InteractionState` type (likely in `engine_observables_api` so the host and the
lane share it).

**Effort / risk:** Medium. The matching machinery and state-change invalidation
already exist, so this is plumbing, not new cascade work. Selection geometry is
the one piece that grows into `FragmentPlane` changes.

**Done when:** a `:hover { color: red }` rule recolors on a host-driven hover
state in a test, `focus_target()` still returns the host-focused node, and the
same state snapshot drives both query read-back and dynamic pseudo-class restyle.

---

## 2. Cascade-time font system (font-relative units `ex` / `ch` / `cap` / `ic`)

**Implemented (2026-06-07).** `font_metrics.rs` resolves the element's font
through a fontique `Collection` and reads metrics with `skrifa` (x-height /
cap-height from OS/2, the `0` advance for `ch`, the U+6C34 advance for `ic`),
scaled by font-size and cached per resolved font. `cascade.rs` wires
`SkrifaFontMetricsProvider` into `Device::new`, so `ex` / `ch` / `cap` / `ic`
now resolve through the matched font rather than Stylo's blind fallbacks. The
collection + cache live in a `thread_local` because the cascade is sequential;
the provider itself is a zero-size `Sync` handle.

**Remaining note:** this uses a cascade-local fontique collection. The next
possible refinement is sharing a single font collection/context with
`TextMeasureCtx` so cascade metrics and text shaping resolve from exactly the
same font registry instance. That is a consistency cleanup, not a blocker for
font-relative units.

**Done when:** covered by the live provider path; keep/extend tests around
`width: 2ch` on a monospace element measuring near two glyph advances rather
than `1em`.

---

## 3. Quirks mode

**Unblocks:** quirks-mode layout for legacy documents (the small set of
quirk behaviors Stylo gates on `QuirksMode`).

**Current blocker:** `LayoutDom` exposes no document quirks mode, so
`adapter_stylo`'s `quirks_mode()` hardcodes `NoQuirks` (~327) and `build_stylist`
passes `QuirksMode::NoQuirks` (~211).

**Approach:** add `fn quirks_mode(&self) -> QuirksMode` to `LayoutDom` (default
`NoQuirks`); `StaticDocument` captures it from html5ever's parse (the tree sink
receives it) and returns it; thread it through `build_stylist` and the adapter.

**Touch-points:** `layout_dom_api` (trait method + default), `serval-static-dom`
(capture + return), `cascade.rs` + `adapter_stylo.rs` (use it).

**Effort / risk:** Small per site, but it fans out. Finding (2026-06-07):
`StaticDocument` *already* captures and exposes the mode (`quirks_mode()` ->
`StaticQuirksMode`, set by its tree sink), so the parse side is free. The
remaining cost is the trait method + ~6 cascade/adapter sites that hardcode
`NoQuirks` (`make_device`, `Stylist::new`, two `MatchingContext`s,
`build_stylist`'s signature, and `adapter_stylo::quirks_mode` which would read
the mode through the TLS cascade context). Near-zero real value (almost all
content is standards mode), so it stays deferred until it rides along with
another `layout_dom_api` / cascade change.

**Done when:** a no-doctype document parses to quirks mode and a quirk-gated
rule (e.g. the table font-size quirk) reflects it.

---

## 4. Pseudo-element cascade (`::before` / `::after` / `::marker` / `::selection` / `::first-line`)

**Unblocks:** generated content (`::before` / `::after` with `content`),
`::marker` styling (closes a list-feature deferral), `::selection`,
`::first-line` / `::first-letter`.

**Current blocker:** the Stylo trait surface for pseudos exists but is stubbed:
`adapter_stylo`'s `match_pseudo_element` ignores its `_pe` argument (~581) and
the cascade entry ignores `_pseudo` (~867). Nothing stores or lays out pseudo
styles, and there is no anonymous-box generation.

**Approach (layered):**
- **Cascade:** implement `match_pseudo_element`; store the computed pseudo styles
  alongside the element (Stylo's `ElementData` already has pseudo slots, so this
  is mostly storage + read-back in `StylePlane`).
- **Box tree:** synthesize anonymous boxes for `::before` / `::after` (driven by
  the `content` property) and `::marker`; this is the bulk of the work and is
  where the list marker would migrate to a real `::marker` box.
- **Paint:** the anonymous boxes paint through the existing path once they exist.

**Touch-points:** `adapter_stylo.rs`, `style.rs` / `cascade.rs` (pseudo storage),
`construct.rs` / `box_tree.rs` (anonymous boxes + `content`), and the list-marker
code (migrate `::marker`).

**Effort / risk:** Large. This is the one genuine multi-part feature here; it
also retires the `list-style-image` and `::marker`-styling deferrals once the
anonymous-box + `content` machinery exists. Land it in slices: cascade/storage
first (no visible change), then `::before` / `::after` content, then migrate the
marker.

**Done when:** `p::before { content: "x" }` paints an `x` before the paragraph.

---

## Cross-cutting note

Items 1, 2, and 4 each touch the cascade adapter (`adapter_stylo`) and
`StylePlane`. If more than one is taken on, do the `StylePlane` storage shape
(pseudo styles, interaction state) as one deliberate change rather than three
incremental ones, so the plane's storage is not reworked repeatedly.
