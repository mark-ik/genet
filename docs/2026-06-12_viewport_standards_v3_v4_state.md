# Viewport standards V3 + V4: state and scope

**Status (2026-06-12):** V1 and V2 of the viewport/root standards work
(`2026-06-12_viewport_root_standards_scope.md`) are shipped and the pelt
viewer is verified on-machine (document scroll, `position: fixed` pinning,
keyboard scroll defaults, anchor-fragment navigation). This note records the
true scope of the two remaining tiers, found while building V1+V2, so each
starts informed rather than from the original "seed fixtures / thin API"
framing.

## The shared engine surface (what V3/V4 consume)

Rule 1 held: the viewport is a first-class per-document object owned by the
session, and every scroll operation is a method on it. These exist today on
`serval_layout::IncrementalLayout`:

- `viewport() -> Viewport`, `viewport_scroll() -> (f32, f32)`
- `scroll_range(dom) -> (f32, f32)` (the scrollable-overflow extent)
- `set_viewport_scroll(dom, (x, y))`, `scroll_by(dom, dx, dy)` (clamped via
  `Viewport::clamp_scroll`)
- `scroll_for_key(dom, ScrollKey)` (the rule-5 keyboard defaults)
- `scroll_to_element(dom, node)`, `scroll_to_id(dom, id)`,
  `link_fragment_at(dom, x, y, scroll)`

Paint + hit-test are viewport-scroll aware (`emit_paint_list_scrolled`,
`scene_from_session_dom`, `ServalLaneView::hit_test`). V3 and V4 are both
"thin over this surface", but in different parts of the tree.

## V3: the pixel reftest harness (a subsystem, not drop-in fixtures)

Found during V1+V2: V3 is meaningfully bigger than "seed fixtures from this
doc". Three reasons (runtime-verified):

1. **The reftest renderer hardcodes no-scroll.** `serval-wpt`'s
   `render::html_to_envelope` passes empty scroll offsets and calls
   `emit_paint_list_with_layouts` (not the scrolled entry). So a document-scroll
   reftest has no way to *trigger* the scroll.
2. **There is no serval-owned fixtures directory.** Every `-ref.html` lives
   under the upstream `tests/wpt/` checkout. V3 stands up a new serval-reftests
   dir + convention.
3. **Scroll genuinely needs pixels.** A scrolled render is a `-scroll`
   *transform wrap*; a statically pre-shifted reference is *shifted
   coordinates*. They converge only after rasterization, so scene / paint-list
   comparison cannot substitute, and the scroll cases need true pixel reftests.

**Enabling first step:** teach the reftest renderer to apply a viewport scroll,
driven by a serval directive (e.g. `<meta name="serval-scroll" content="x y">`)
feeding `emit_paint_list_scrolled`.

**Fixture set (from the scope doc):** document-scroll-offset (root- and
body-propagated), `overflow: hidden` on root disabling scroll, fixed-vs-absolute
under a scrolled viewport, the `%`-height chain (static, reftestable without the
scroll extension), and a scrollable-overflow-region case with an abs-pos
overhang.

**Counterweight:** the scroll family is already locked at the paint-list level
by the V1+V2 unit tests (propagation both halves, paint scroll, fixed counter,
hit-test, clamp, range, keyboard defaults, anchor nav). V3 adds *pixel / GPU*
paint confidence on top, a lower-probability regression class. Deferred as a
deliberate harness effort, not a blocker.

## V4: the scripted scroll API (engine-ready, scripting-layer net-new)

V4 is the JS surface: `window.scrollTo` / `scrollBy` / `scrollX|Y`,
`Element.scrollIntoView`, `document.scrollingElement`, and scroll events.

- **Engine substrate: ready.** Each maps thin onto the session methods above
  (`scrollTo` -> `set_viewport_scroll`, `scrollBy` -> `scroll_by`,
  `scrollX|Y` -> `viewport_scroll`, `scrollIntoView` -> `scroll_to_element`).
  The scope doc's "all thin over the viewport object if rule 1 held" is true.
- **API surface: entirely net-new.** No `scrollTo` / `scrollBy` /
  `scrollIntoView` / `scrollingElement` / `scrollX|Y` / scroll-event binding
  exists anywhere in serval. V4 is all new bindings.
- **Lives in the scripting subsystem** (`script-runtime-api`, `serval-scripted`,
  `script-engine-api` + the Boa/Nova engine), **not pelt** (script-free) and not
  serval-layout. It is a different layer than the V1-V3 viewer work.
- **Not blocked** (unlike scroll-padding below). The pieces exist; the work is
  the JS binding plumbing plus one genuinely new mechanism: **scroll-event
  dispatch** (fire `scroll` on the document/element when the viewport moves),
  through the scripted-DOM event path.
- **`scrollingElement`** needs the root (standards) / body (quirks) branch;
  quirks mode already landed (2026-06-11), so the data is there.

V4's natural host is meerkat / the scripted content path, where the JS engine
and DOM bindings live. pelt stays script-free.

## Named deferral (blocked, on the record)

`scroll-padding` / `scroll-margin` (the fixed-header anchor offset): the spec
mechanism, but serval's stylo build does not compile those CSS Scroll Snap
longhands (verified absent from the `Position` / `Padding` / `Margin` / `Box`
computed structs), so there is nothing to read. Bare block-start stays the spec
default. Documented on `IncrementalLayout::scroll_to_element`; revisit when
stylo gains the properties.
