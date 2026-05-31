# serval two development lanes (2026-05-29)

Operationalizes the [holistic audit](./2026-05-29_serval_holistic_audit.md):
its two converging arcs become two explicit development lanes plus the shared
spine they both stand on. This is a planning structure, not new architecture.

**Terminology note.** "Lane" is already used in serval for **Hekate
engine-routing** lanes (Nematic / Serval / Scrying;
[2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md)),
which decide *which engine handles a source*. These two development lanes are
a **different axis**: parallel *workstreams inside Serval*, not routing
targets. When ambiguity matters, say "development lane" vs "Hekate lane".

## The shape: two lanes, one spine

```text
          Content lane                         Host lane
   (serval runs the real web)        (serval hosts the app, chrome as CSS)
   WPT / JS / DOM breadth            xilem-serval / native authoring
            \                                      /
             \                                    /
              \------------- shared spine -------/
        one DOM (serval-scripted-dom + LayoutDomMut)
        one event model (capture/target/bubble)
        one render path (Stylo + box-tree + parley + netrender)
        IncrementalLayout relayout
```

The lanes diverge at their *consumers* (page JS vs Rust app state) and
converge at the substrate. The discipline: spine work is built once and both
lanes consume it; a lane never forks the spine to move faster. The on-strategy
test from the audit holds: the highest-value items are the ones that serve the
spine (and therefore both lanes) at once.

## Lane C — Content (serval runs the real web)

**Mission.** Load and run real web content faithfully: HTML + CSS + JS, scored
by WPT, scaling up the profile ladder toward fullweb.

**Current state.** Scripting tier is a working conformance loop: pluggable
engines (Nova native, Boa wasm/oracle) behind `ScriptEngine`/`CallCx`;
`script-runtime-api` host surface; WPT runner phase 3. html/dom 35,366
subtests, dom/nodes 832 (Boa). Reftests render real CSS (floats 7 passing).

**Backlog (ordered).**

1. ~~**Exotic-object primitive**~~ **(done 2026-05-30).** Settled via JS
   `Proxy` (both engines support the traps), not a new `CallCx` primitive:
   live HTMLCollection / NodeList, DOMTokenList (incl. indexed), `dataset`.
2. ~~**Broader `DOMException`-throwing on bad input**~~ **(done 2026-05-30).**
   `createElement` / `setAttribute` validate the XML Name production
   (`InvalidCharacterError`); `createElementNS` / `setAttributeNS` validate
   QName + namespace constraints (`NamespaceError`); `appendChild` /
   `insertBefore` reject ancestor cycles (`HierarchyRequestError`). Fixed
   two latent bugs: HTML `createElement` lowercasing, and `tagName`
   returning the qualified name (`prefix:local`).
3. **`Comment` / `CharacterData`** node types **(done 2026-05-30).** The
   `CharacterData` → `Text` / `Comment` prototype chain (`instanceof`,
   `data` / `length`, `appendData` / `insertData` / `deleteData` /
   `replaceData` / `substringData` with `IndexSizeError`), `new Text()` /
   `new Comment()`, `splitText` / `wholeText`, and Node identity
   (`isEqualNode` / `compareDocumentPosition` + constants / `isConnected` /
   `isSameNode` / `ownerDocument` / `getRootNode`). Also fixed the
   enumerated reflected-attribute getter to a real keyword table
   (limited-enum with `""` missing-value default), starting with the
   global `dir`. `DocumentFragment` remains.

   **DocumentFragment + cloneNode + enum tables (done 2026-05-31),
   finishing the bounded DOM-API set.** `NodeKind::DocumentFragment`
   (nodeType 11) in the DOM crate + `create_fragment`; the JS
   `DocumentFragment` prototype, `createDocumentFragment`, and the
   `ParentNode` query mixin (`querySelector(All)` / `getElementById`)
   scoped to it. `Node.cloneNode(deep)` (shallow copies element
   attributes / namespace; deep recurses the subtree) over the existing
   create* primitives. The enumerated keyword table extended to the
   conflict-free per-element enums (`referrerPolicy`, `crossOrigin`,
   `decoding`, `method`, `enctype`, `loading`, `preload`, `kind`,
   `inputMode`, `enterKeyHint`, `autocomplete`, `scope`; `type` /
   `formMethod` skipped — multiple keyword sets across interfaces).
4. ~~**`createDocument` / `createHTMLDocument`**~~ **(done 2026-05-30)** with
   the multi-document increment (a second detached document root in the
   host, scoped queries, `document.implementation`). **`DOMParser.parseFromString`**
   still open (reuses that same plumbing).
5. **Per-tag HTML element interfaces** (`HTMLElement` + `HTMLDivElement` ...,
   `wrapNode` picks prototype by tag) for `instanceof` and `cloneNode`.
6. **Cross-engine breadth**: the Nova regex gap (surrogate ranges in
   testharness sanitize), then QuickJS as a third backend if wanted.
7. **Fullweb tier** later (navigation, workers, storage), per the profile
   ladder, gated on real demand.

**Completeness axes.** The backlog above is largely the *scripting / DOM-API*
axis (testharness-scored). "Run the real web faithfully" has parallel axes the
backlog had not enumerated; each fails for different reasons, is reftest- or
testharness-scored, and ratchets independently. Naming them so CSS/rendering
work does not fall through the gap between "DOM-API backlog" and "measured by
WPT":

- **Layout** — where boxes go. Done: block, flex, grid, floats, replaced
  `<img>`, the planes model + restyle damage. Missing: `display: inline-block`
  and `table`/multicol, `position` (relative/absolute/fixed/sticky),
  overflow/clipping, writing-modes, the long tail of sizing/units. `inline-block`
  extends the existing inline path rather than adding a mode from scratch: lay
  the element out via taffy for its intrinsic size, then feed it into the parent
  inline formatting context as an `InlineBoxItem` on the same seam `<img>` uses
  (`InlineContent` / opaque-leaf, per
  [blitz float/linebox study](./2026-05-20_blitz_float_linebox_study.md)).
  Reftest-scored.
- **Paint / visual styling** — whether each *computed* property actually
  *renders*. Cascade resolves the values (Stylo, largely free); paint emission
  must draw them. Done + e2e-tested (`html_to_pixels_e2e`): backgrounds, borders,
  box-shadow (hard + blur), background-image tiling, `<img>`, text-glyph
  rasterization. Missing: `border-radius`, gradients, `opacity`, clipping,
  `transform`, z-index/stacking. Distinct from layout (placement vs. appearance).
  Reftest-scored. **Correction (2026-05-31):** the audit's "text-to-pixels gap"
  was stale — text rasterizes and passes its e2e test. The real headline gap is
  the **reftest pass rate** (floats 7/197, css-backgrounds 15/1326): rendering
  works in isolation but fails pixel-compare across the corpus. That systematic
  gap is the [CSS rendering conformance plan](./2026-05-31_css_rendering_conformance_plan.md).
- **Text / typography** — shaping breadth (bidi, complex scripts, font
  fallback), `white-space` / line-breaking / `text-overflow`, `@font-face` / web
  fonts. parley-backed; the glyph-runs-to-pixels translator is the immediate
  gap (shared-spine, below).
- **Accessibility (content-side)** — the `DOM → AccessKit` emission (the
  `accesskit_tree` builder, Lane H item 5) applied to *content* documents, so
  real pages expose an a11y tree, not just chrome. Same primitive, content
  consumer; reinforces the R0 a11y contract.
- **Animations / transitions** — CSS transitions/animations and the Web
  Animations API: interpolated styles (Paint axis) driven by a timing model
  (`requestAnimationFrame`, the event loop). Shared with Lane H chrome
  animation; a later tier.
- **Parsing / input formats** — what serval can turn into a `LayoutDom`.
  Done: HTML5 (`html5ever` → `StaticTreeSink`). Wanted: **XHTML** (Mark, 2026-05-31:
  in scope). `xml5ever` is *already a workspace dependency* (root `Cargo.toml`,
  0.39, same `markup5ever` interface as html5ever) but unused; the cheap path is
  to drive serval-static-dom's existing `StaticTreeSink` with `xml5ever::parse_document`
  and route `.xht`/`.xhtml` (and `application/xhtml+xml`) to it — reusing the sink,
  not writing a parser. Unlocks the large CSS2 `.xht` reftest corpus (961/1045 of
  normal-flow), so it is the natural first lever of the rendering-conformance plan.
  The runner's current `.xhtml` *skip* becomes a temporary measure, not permanent.
- **Security (later)** — same-origin enforcement, CSP, sandboxing, and
  mixed-content beyond netfetcher's network-side checks. Real for arbitrary web
  content; a future tier, named so it is not forgotten rather than scheduled now.

**Real-world conformance targets (Lane C litmus, not features).** Beyond WPT,
named libraries are end-to-end proofs of scripted+fullweb completeness. **htmx**
(Mark, 2026-05-31: would be cool) is the near-ideal first target — it is a
*library* (`hx-*` attributes + `fetch` + DOM swapping), so "serval runs htmx
unmodified" exercises DOM-API breadth (mostly there), `fetch`/XHR (the netfetcher
organ, not yet wired to script), `history`/events, and attribute observation.
Track it as a milestone the way `testharness.js` was the host-surface litmus, not
as a thing to implement directly.

Adjacent and *already owned elsewhere*, so cross-referenced not duplicated:
resources/media (netfetcher = network, net-media = a/v — organs this lane
consumes; SVG / `<canvas>` are Lane-C content but later); navigation / workers /
storage (the fullweb tier, item 7 above). Forms are cross-cutting — element
interfaces fall under DOM-API, sizing under Layout, rendering under Paint, and
host-authored controls under Lane H — not a separate axis.

**Done-conditions.** Per-directory, per-backend WPT pass rates published and
ratcheted; the Nova-vs-Boa delta read as the engine-axis tax (the two-axis
framing). Owner docs:
[pluggable-engines](./2026-05-26_pluggable_engines_testharness_plan.md),
[web-platform-API shared middle](./2026-05-25_web_platform_api_shared_middle_plan.md),
[WPT runner](./2026-05-26_wpt_runner_plan.md),
[JS execution strategy](./2026-05-25_js_execution_strategy.md).

## Lane H — Host (serval hosts the app)

**Mission.** Author Mere's chrome (and eventually content shells) as documents
in serval, via the `xilem_serval` reactive backend: Rust app state to a view
tree to DOM mutations, serval the sole engine. Architecture 3 from the host
doc.

**Current state.** Implemented through Stage 2 + on-screen demo: `xilem_core`
backend over `ScriptedDom`, `ServalAppRunner`, faithful event dispatch
(capture/target/bubble), keyboard + focus, form-control start, caret editing.
`pelt-live-counter` runs a reactive counter on screen with serval as the only
engine.

**Backlog (ordered), from the host doc Stage 3.**
1. **Element / Text view split** (wrappers are uniform `Node` today). Draws on
   Content lane's reflection/traversal work already landed.
2. **Action bubbling / `OptionalAction`** so handlers compose up parent views.
3. **Wider event + view vocabulary** (`pointermove` / `pointerup`, more
   element views, per-tag ergonomics).
4. **Form controls** (the genuine engine-completeness cost; shared with
   Content lane and fullweb).
5. **DOM-to-AccessKit** accessibility emission from the semantic DOM (more
   natural here than from a widget tree).
6. **Separate-document authority** between chrome (xilem-serval-owned root)
   and content (page-JS-owned root), made real in code rather than asserted.

**Completeness axes.** The backlog above is the *authoring + event + forms +
a11y* axis (the reactive layer and its handlers). "Author the chrome as
documents" needs things a document engine does not give for free the way a
widget toolkit would; naming them so they are designed, not discovered:

- **Overlays / popups** — dropdowns (`<select>`), context menus, tooltips,
  modals/dialogs. A document engine has no layer above the flow; serval-as-host
  must provide one, either in-document (needs Lane C's `position` + stacking) or
  as separate surfaces (true popups / multi-window). `<select>` in form breadth
  already requires it.
- **Pointer gestures / drag-and-drop** — drag-to-resize / rearrange / tear-out
  (platen tiling), canvas pan/zoom (the orrery camera). A gesture layer over raw
  `pointerdown` / `move` / `up` (item 3): drag threshold, drag state, drop
  targets — above click/key.
- **Scrolling / overflow interaction** — scrollable panes/lists, scrollbars,
  wheel/scroll events, the infinite-canvas navigation defaults (wheel=pan,
  ctrl+wheel=zoom, inertia). overflow *layout* is Lane C; the *interaction* is
  here.
- **Styling / theming authoring** — "chrome as CSS" made real: where chrome
  stylesheets live, design tokens, dark/light + runtime theme swap, and how an
  `xilem_serval` view sets style (a `class` + host sheet today). Appearance
  renders through Lane C's Paint axis; *authoring* it is Lane H's.

Adjacent / owned elsewhere: multi-window with synced panels and the
chrome-update perf spike (transform-only motion on the `RepaintOnly` path), both
in the Mere host brief; animations/transitions in the shared spine (below).

**Done-conditions.** Real Mere chrome (a panel, a shellbar) authored end to end
through `xilem_serval` with serval as the sole engine, input and a11y working.
Owner doc:
[serval-as-host](./2026-05-27_serval_as_host_xilem_serval_plan.md).

## The shared spine (built once, consumed by both)

This is where the leverage is, and where forking is the risk.

- **One event model.** Native dispatch (Host lane) and JS-listener dispatch
  (Content lane, `script-runtime-api/dom.rs` W0c) must run *one*
  capture/target/bubble algorithm with two entry points, per the host doc's
  explicit mandate. This is the convergence most at risk of silent divergence
  and the first spine item to settle.
- **The DOM mutation + query API** (`LayoutDomMut`, hit-test). Each method is
  dual-use; `insert_before` / `remove_attribute` / `remove_child` already
  proved it.
- **The exotic-object `CallCx` primitive.** A `ScriptEngine`-trait extension
  implemented per backend (Nova, Boa). Content lane needs it for live
  collections; Host lane benefits from the same object machinery. One
  primitive, both lanes.
- **Text-to-pixels.** Glyph runs to the netrender translator. Makes both lanes
  visible; today everything lays out but text does not show.
- **`IncrementalLayout` + netrender output.** The relayout-and-present spine
  both lanes already share.

**Spine rule.** A spine change lands in the shared crate
(`serval-scripted-dom`, `serval-layout`, `script-engine-api`,
`script-runtime-api`) with both lanes' use in view, never duplicated into a
lane. The event model in particular converges, it does not fork.

## How the lanes interleave

They mutate the same DOM surface, so they cannot run as truly independent
parallel edits (the audit's serial-on-the-substrate finding). The practical
cadence:

1. **Spine first when a piece is shared.** The audit's top three
   (event-dispatch convergence, text-to-pixels, exotic-object primitive) are
   all spine work; doing them serves both lanes and clears each lane's next
   step.
2. **Then lane-local breadth in parallel-in-time** (different sittings, same
   substrate): Content lane grows DOM/JS breadth against WPT; Host lane grows
   view/event vocabulary against on-screen demos. These touch mostly different
   files (`script-runtime-api` vs `xilem-serval`) once the spine is in place.
3. **Each lane has its own scoreboard**: WPT subtest counts (Content), an
   on-screen authored chrome surface (Host). Spine work shows up as lifts in
   both.

## Why not collapse them into one

Keeping the lanes named keeps two failure modes visible. Content lane drifting
toward web-completeness for its own sake (boiling the WPT ocean) is checked by
asking whether a feature also serves the host. Host lane reaching for
"architecture 2" (serval chrome on a foreign host framework, two of
everything) is checked by the host doc's standing exclusion. Two lanes, one
spine, with the spine as the arbiter of what is worth doing.
