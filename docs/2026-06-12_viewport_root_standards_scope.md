# Viewport & root special rules: the standards scope

**Status (2026-06-12):** scouting scope, for the pelt V1+ work and
genet-layout. Prompted by the document-scroll gap (a page taller than the
window scrolls with zero CSS; genet only scrolls element overflow
containers). That gap has a *shape*, and this doc maps the rest of the
family so the engine grows standards-compliant mechanisms instead of
per-host hacks.

**The pattern:** CSS and HTML give the root/viewport pair a family of
special rules — propagation rules (root's X becomes the viewport's X),
attachment rules (some boxes anchor to the viewport, not their ancestors),
and UA default *actions* (wheel and keys scroll the document). An engine
built element-first misses them in a correlated way, and hosts are then
tempted to fake each one with element machinery ("make the root an overflow
container and key it"), which inverts the model and fights pages that set
their own overflow. genet already proves the right shape once:
**canvas-background propagation is fully implemented, both halves** (root →
canvas, with the HTML body fallback and display-contents/none negative
tests; `paint_emit.rs:1325-1362`). Every fix below is "do for X what was
done for background."

**Verified current state (2026-06-12):**

- Scrolling is element-only: `ScrollOffsets<NodeId>` keyed by element ids,
  threaded through paint/hit-test/incremental; no viewport entry; "only a
  clipping container scrolls."
- `position: fixed` is treated identically to `absolute`
  (`paint_emit.rs:418` lifts both the same way): no viewport attachment.
  Invisible today precisely *because* there is no document scroll; breaks
  loudly the moment there is.
- `sticky` exists only in positioned-ness checks; no scroll attachment.
- Canvas-background propagation: complete (the model to copy).
- Quirks mode landed (2026-06-11), which the `scrollingElement` rule needs.

**Update (2026-06-14): the V1 viewport family landed; this scope is largely
discharged.** Verified against the code (a parallel capability audit, not this
doc's prior snapshot):

- **Document / viewport scroll: DONE.** A first-class `Viewport` (`viewport.rs`)
  owns offset + propagated overflow + size; root → viewport propagation with the
  `<body>` fallback and `overflow:hidden`-disables-scroll (rule 2, both halves);
  `document_scroll_range` from the scrollable-overflow region (rule 4); the
  session emits the document at `-scroll` and hit-tests through `+scroll`. Tested
  in `layout::tests` + `incremental::tests`.
- **`position: fixed`: DONE** (rule 3). `is_fixed` / `attaches_to_viewport`; the
  stacking layer counters the document scroll so a fixed box stays pinned in
  *both* paint and hit-test (`Fixed ≠ Absolute`); tested.
- **`%`-height chain, viewport units (`vw`/`vh`), resize re-resolution: DONE**,
  guarded in `layout::tests`.
- **Inline-box hit-testing + `pointer-events`: DONE** (2026-06-14) — the
  hit-test family this scope is adjacent to. See
  `2026-06-14_engine_capability_audit.md`.
- **Still open (the one V1-adjacent gap):** nested (element)
  `overflow:scroll/auto` scrolling. The paint + hit consumers already read
  `ScrollOffsets`; nothing populates the per-element map (it is always
  `::default()`). A data-flow gap, not an algorithm gap; the next slice.
- `sticky`, `background-attachment:fixed`, and the named knockouts remain
  deferred as scoped.

---

## Engine model rules (the guidance, in one place)

1. **The viewport is a first-class per-document object** — scroll position,
   size, propagated overflow, the canvas — not "the window." meerkat's
   content cards and future iframes are each a document with its own
   viewport, so building it per-document once serves every host.
2. **One scroll-container abstraction; the viewport is its root instance.**
   Document scroll arrives via root → viewport *overflow propagation* (CSS
   Overflow §3), with **both halves** like the background sibling: the root
   element's overflow propagates to the viewport; when the root's is
   `visible`, the HTML `<body>`'s is consulted; `overflow: hidden` on the
   root disables viewport scroll. `ScrollOffsets` gains a viewport slot —
   never a faked root-element key.
3. **Scroll attachment is a tree property, not a position style.** `fixed`
   boxes attach to the viewport (do not move under document scroll; hit-test
   unscrolled); `sticky` attaches to the nearest scrollport. The paint walk
   and hit walk both need the distinction `Fixed ≠ Absolute`.
4. **Scroll bounds come from the spec, not from "content height."** The
   scroll range is the viewport's *scrollable overflow region* (CSS Overflow),
   which includes abs-pos and transformed descendants' overflow.
5. **UA default actions live once, engine-adjacent, not per host.** Wheel
   scrolls the nearest scrollable ancestor of the hit point *including the
   viewport*; Space/Shift+Space/PageUp/PageDown/Home/End/arrows scroll the
   document when focus is not in an editable; anchor-fragment navigation
   (`#id`) and focus-into-view scroll to elements. One shared helper that
   pelt and meerkat both call; never two hand-rolled copies.
6. **When a root/viewport rule turns up missing, look for its siblings**
   before fixing it alone — this family travels together (this doc is the
   sibling list).

## The case checklist, tiered by when pelt hits it

### V1 (static viewer) — these block "renders and scrolls a real document"

- **Viewport/document scroll** via rule 2. Touch-points: a per-document
  viewport object beside `ScrollOffsets`; the propagation read mirrors
  `emit_canvas_background`'s source choice (root cv, else body cv); paint
  translates in-flow content by the viewport offset inside the canvas;
  hit-testing adds the offset; `IncrementalLayout` carries the viewport slot
  through its retained planes. The host feeds wheel deltas to the shared
  default-action helper (rule 5) instead of routing scroll itself.
- **`position: fixed` viewport attachment** (rule 3): excluded from the
  scrolled translation, hit-tested unscrolled. Land *with* document scroll,
  not after — shipping scroll with Fixed≡Absolute is a visible regression on
  real pages.
- **The %-height chain**: `html`/`body { height: 100% }` resolves against
  the initial containing block (viewport-sized). The classic element-first
  miss; verify with a fixture rather than assuming taffy's root sizing does
  it.
- **Viewport units** (`vw`/`vh`/`vmin`/`vmax`): stylo resolves via `Device`
  (which genet already sizes); verify resize re-resolves (a resized window
  re-cascades or at least re-resolves lengths). The `sv*`/`lv*`/`dv*`
  variants are a named deferral, not a v1 need.
- **`background-attachment: fixed`**: same family (paints
  viewport-anchored); cheap once the viewport object exists; fine to defer
  *explicitly*.

### V2 (chrome) — default actions and navigation

- Keyboard scrolling defaults (rule 5's key list) through the shared helper.
- Focus-into-view on Tab traversal (composes with the grabbag plan's
  Tab work).
- Anchor-fragment navigation: `pelt url#id` and in-page link clicks scroll
  to the element.

### V3 (reftest harness) — seed fixtures from this doc

First fixtures after the pseudo-element slices: document-scroll offset
scenes (root-propagated and body-propagated), `overflow: hidden` on root
disabling scroll, fixed-vs-absolute under a scrolled viewport, the %-height
chain, and a scrollable-overflow-region case with an abs-pos overhang. The
harness exists to catch exactly this family regressing.

### V4 (scripted) — the API surface over the same model

- `document.scrollingElement` (root in standards mode, body in quirks —
  quirks support already landed).
- `window.scrollTo`/`scrollBy`/`scrollX|Y`, `Element.scrollIntoView`,
  scroll events. All thin over the viewport object if rule 1 held.

### Named knockouts (defer deliberately, on the record)

`position: sticky` (needs the scroll tree; schedule after document scroll
settles), `scroll-behavior: smooth` / `overscroll-behavior` / scroll-snap
(all hang off the one scroll-container type — cheap *later* precisely
because of rule 2), writing modes + logical viewport units, viewport meta /
pinch zoom, paged media.

### Adjacent audit item (same hack-vector, different mechanism)

**UA stylesheet completeness** against the HTML spec's Rendering section.
Every missing UA default (body margin, hidden attribute, replaced-element
`aspect-ratio` from `width`/`height` attributes, form-control baselines)
shows up as a host "fixing" rendering in author CSS — the same
paper-over-the-engine failure mode this doc exists to prevent. One pass,
one fixture per default adopted.

---

## Why this doc exists

Pelt's role is to surface exactly these gaps (a reference shell can't hide
engine holes behind product machinery, which is how the scroll gap stayed
invisible until the counter demo "never scrolled the page"). The rule of
engagement it enforces: **when pelt hits a rendering gap, the fix lands in
genet-layout as the spec's mechanism, and the host change is limited to
feeding inputs to it.** If a fix only works for pelt, it isn't the fix.
