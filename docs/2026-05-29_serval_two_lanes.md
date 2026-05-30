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
1. **Exotic-object `CallCx` primitive** (shared-spine; see below). Unblocks
   live HTMLCollection / NodeList / `dataset` / indexed DOMTokenList, the
   largest remaining WPT bucket after reflection.
2. **Broader `DOMException`-throwing on bad input** (`setAttribute` invalid
   names, `createElementNS` validation, index ranges) so `assert_throws_dom`
   tests pass, not just run.
3. **`Comment` / `DocumentFragment` / `CharacterData`** node types and
   `createComment` / `createDocumentFragment`.
4. **`DOMParser` / `createHTMLDocument`** (needs a second detached document
   root in the host).
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
  must draw them: backgrounds, borders + `border-radius`, `box-shadow`,
  gradients, `opacity`, clipping, `transform`, z-index/stacking. Distinct from
  layout (placement vs. appearance). Reftest-scored. The headline
  `text-to-pixels` gap is the text-shaped corner of this.
- **Text / typography** — shaping breadth (bidi, complex scripts, font
  fallback), `white-space` / line-breaking / `text-overflow`, `@font-face` / web
  fonts. parley-backed; the glyph-runs-to-pixels translator is the immediate
  gap (shared-spine, below).
- **Security (later)** — same-origin enforcement, CSP, sandboxing, and
  mixed-content beyond netfetcher's network-side checks. Real for arbitrary web
  content; a future tier, named so it is not forgotten rather than scheduled now.

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
