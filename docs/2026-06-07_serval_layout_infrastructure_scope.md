# serval-layout: infrastructure scope (post emit-feature push)

**Status (2026-06-11): ALL FOUR ITEMS DONE — plan complete, ready to archive.**
The 2026-06-07 scope identified four infrastructure pieces every remaining
deferred-inventory item needed. All four have since landed:

| # | item | status |
| --- | --- | --- |
| 1 | Interaction state (`:hover`/`:focus`/…) | **DONE** `a2281b3d0fc` |
| 2 | Cascade-time font system (`ex`/`ch`/`cap`/`ic`) | **DONE** (2026-06-07) |
| 3 | Quirks mode | **DONE** `03baa49fbd9` |
| 4 | Pseudo-element cascade | **DONE** — see [pseudo follow-ups](2026-06-11_pseudo_element_followups_scope.md) |

The doc below is the original scope, with each section's status reconciled to the
code (items 1 and 3 finished *after* this doc was written, so they read stale in
older revisions). **One low-value cleanup remains**, not blocking anything: item
2's shared font-collection note (cascade metrics + text shaping from one fontique
`Collection`). The reftest scoreboard lives in the conformance plan, not here.

Original recommended order (by value over effort): **interaction state** (mostly
wired, high value), **font system** (self-contained), **quirks mode** (small),
then **pseudo-element cascade** (largest, but unblocks the most).

---

## 1. Interaction state (`:hover` / `:focus` / selection / focus queries)

**Status (2026-06-11): DONE (`a2281b3d0fc`).** The cascade source that was the
gap below landed: a host-owned `InteractionState`
(`engine_observables_api::interaction.rs`) is threaded in via `apply_interaction`
/ `restyle_for_interaction` (cascade.rs), which populate each affected
`StyleEntry::state` (`StylePlane::apply_interaction_bits` /
`set_element_state`, with `add_interaction_chain` for `:hover`/`FOCUS_WITHIN`
ancestor chains) and run Stylo's state-change invalidation for the minimal
restyle. `:checked` rides the same path (`e1fb3680db9`). Tests:
`interaction_hover_drives_restyle` (the done-condition: a host hover recolors,
moving it reverts the old node + recolors the new, via the snapshot path not a
full re-cascade) and the `p:hover` recolor test. Selection-range geometry — the
sub-task split out below — also shipped as `range_rects` (pseudo follow-ups §3,
`41147890b21`). The rest of this section is the original scope, kept for context.

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

**Status (2026-06-11): DONE (`03baa49fbd9`).** `LayoutDom::quirks_mode()` now
exists (defaulting `NoQuirks`); `StaticDocument` returns the mode its tree sink
captured from html5ever, and it threads through `build_stylist` / `make_device`
/ the adapter via `selectors_quirks_mode`. Guard:
`quirks_mode_flows_from_parser_to_stylist`. (Low real value as predicted —
almost all content is standards mode — but it rode along cleanly with the
adapter work rather than waiting.) The rest of this section is the original
scope.

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

**Status (2026-06-11): the tractable slice shipped; remainders re-scoped.**
Inline `::before`/`::after` content (`0694833a3ed`) and `:checked`
(`e1fb3680db9`) landed — the cascade resolves eager pseudos for free, as this
section predicted. The five remainders (::marker lazy resolution, ::selection
host wiring, block-display generated content, ::first-letter, selection-range
geometry) each hit a verified wall and are scoped individually in
[`2026-06-11_pseudo_element_followups_scope.md`](2026-06-11_pseudo_element_followups_scope.md),
which supersedes the approach sketch below for pseudo-elements.

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

**Retrospective (2026-06-11):** in the event the plane's storage held up — both
interaction state (`StyleEntry::state`, item 1) and pseudo styles (eager slots +
the lazy `::marker` map, item 4) landed as additive fields without reworking the
plane between them, so the feared repeated rework did not materialize.

## Lone remaining cleanup (not blocking)

Item 2's shared-font-collection note: the cascade resolves font metrics through
its own thread-local fontique `Collection`, while text shaping uses
`TextMeasureCtx`'s. Sharing one `Collection` instance between them is a
consistency cleanup (one registry, no double discovery), not a correctness or
cost issue — the cascade's metrics collection is already amortized. Pick it up if
a font-registry divergence ever surfaces; otherwise this plan is done and
archivable.
