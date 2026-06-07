# serval-layout: infrastructure scope (post emit-feature push)

**Status (2026-06-07):** scoping, for review.

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
`:focus-within`, `:checked`, …) actually changing style; the `ServalLaneView`
`InteractionQuery` stubs (`focus_target`, `selection`, `affordances_at`,
`activation_target`); selection-range geometry.

**Current state (already built):** `StyleEntry` carries a `state: ElementState`
field, and `adapter_stylo`'s `match_non_ts_pseudo_class` already matches the
state-backed pseudo-classes against it (`adapter_stylo.rs` ~563, `state()` at
~727). So the *matching* half is done. What is missing is the *source* of that
state and the *read-back* of it.

**Approach:**
- A host-owned `InteractionState` (focused node, hover/active node chain,
  selection range), passed into the restyle path. Before a restyle, populate
  each affected `StyleEntry::state` from it (set/clear `HOVER` / `FOCUS` /
  `ACTIVE` / `FOCUS_WITHIN` on the right nodes), then let Stylo's existing
  state-change invalidation restyle the minimum.
- `ServalLaneView` gains a borrow of the same `InteractionState` so
  `focus_target` / `selection` / `activation_target` answer from it instead of
  returning `None`. `activation_target` walks the hit node up to the nearest
  focusable/`<a>` ancestor.
- Selection-range geometry (`rects_for_selection`) is a separate, larger sub-task
  (needs line-box rects threaded into the `FragmentPlane`); split it out.

**Touch-points:** `cascade.rs` (state population + invalidation), `serval_lane.rs`
(query answers), a new `InteractionState` type (likely in `engine_observables_api`
so the host and the lane share it).

**Effort / risk:** Medium. The matching machinery and state-change invalidation
already exist, so this is plumbing, not new cascade work. Selection geometry is
the one piece that grows into `FragmentPlane` changes.

**Done when:** a `:hover { color: red }` rule recolors on a host-driven hover
state in a test, and `focus_target()` returns the host-focused node.

---

## 2. Cascade-time font system (font-relative units `ex` / `ch` / `cap` / `ic`)

**Unblocks:** correct `ex` / `ch` / `cap` / `ic` units (today they resolve to
Stylo's blind fallbacks, e.g. `ex = 0.5em`).

**Current blocker:** `cascade.rs` builds the device with the stub provider:
`Device::new(..., Box::new(StubFontMetricsProvider), ...)` (~line 148), which
returns `FontMetrics::default()` for every query. Approximating from `font_size`
alone reproduces Stylo's existing fallbacks, so it adds nothing; real metrics
require the actual matched font.

**Approach:** back the provider with a font collection (the same fontique
`Collection` parley uses, shared so both agree). `query_font_metrics` resolves
the `font_styles` + size to a font and reads swash metrics (x-height, cap-height,
the advance of `0` for `ch`, the advance of the CJK water ideograph for `ic`),
scaled to the size. Thread the provider into `make_device` / `build_stylist` /
`run_cascade`.

**Touch-points:** `font_metrics.rs` (real provider), `cascade.rs` (thread it),
shared font collection (parley's `FontContext` is created later in
`TextMeasureCtx`; the cascade would need its own or a shared handle).

**Effort / risk:** Medium. Two findings from a closer look (2026-06-07):

- The cascade is **sequential** (`cascade.rs` ~157, `driver::traverse_dom` with
  no thread pool), so the thread-safety concern is moot for now: the provider is
  used single-threaded.
- serval-layout depends only on `parley`, which does **not** re-export `swash`
  or the fontique `Collection`. So this needs a new font-parsing dependency
  (`swash` / `skrifa` / `read-fonts`) plus access to a fontique `Collection` to
  resolve a family and read its metrics. That is a dependency decision, and the
  value (font-relative units are rare in real CSS) is low, so it was deferred
  rather than adding a font-parser unprompted. The natural shape is to construct
  one shared `FontContext` up front and hand it to both the cascade provider and
  `TextMeasureCtx`, so they agree and the collection is built once.

**Done when:** `width: 2ch` on a monospace element measures ~2 glyph advances,
not `1em`.

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

**Effort / risk:** Small, but cross-crate. Low value (almost all real content is
standards mode); do it opportunistically, e.g. alongside another
`layout_dom_api` change.

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
