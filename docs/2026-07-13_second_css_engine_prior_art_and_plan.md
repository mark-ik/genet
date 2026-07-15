# A second CSS engine: prior art and growth plan

**Date:** 2026-07-13
**Status:** E0a full-path audit, E0b Cambium lane choice/40-property catalog,
E1 values/codegen, E2 style resolution, and the first E3 integration slice are
landed. The catalog has since ratcheted to 87 properties. `genet-livery` now adapts `LayoutDom` into Livery's selector and cascade
path, retains a concrete Livery style plane, lowers the bounded box model into
a standalone Taffy tree, and emits backgrounds and borders through the neutral
`PaintList` API. Consecutive text and inline-element children now shape in one
Parley context, while a retained `LiveryDocument` owns the text system, stable
font resources, and cached frame. The opt-in `genet.livery` session route
lowers that PaintList into `netrender::Scene`; the existing default remains
`genet.web`. The integration crate's normal/build dependency graph excludes
Stylo. Parley's positioned lines now supply multi-fragment span paint geometry
and atomic `inline-block` placement. The retained interaction path now covers
viewport scroll, pointer-events hit testing, link rectangles, fragment
navigation, focus state, rounded fills, and two-stop gradient layering. A
host-driven opacity clock supplies intermediate frames, and bounded
`transition-property`/`transition-duration` metadata starts opacity and
background-color transitions on that same clock, so `transition: all` can paint
both changes in one retained tick. Nested scroll containers now route wheel deltas
into retained offsets, chain at their boundary to the viewport, and replay
descendant paint through transforms. Bounded opacity-only `@keyframes` and
named timing functions now run on the retained clock. Remaining E3 work is
remote resource loading, replaced-element image layout, full WPT reftest parity,
broader transition-property lists and interpolation, and the cold-build
comparison. The
first audit invalidated the proposed 33-accessor full-crate seam.
The second chose Cambium structural UI as the bounded first lane.
Mark's framing: "grow a rust alternative using firefox, chrome, servo,
blitz, ladybird, and gosub as prior art... think it'd be neat to have two
css engines. seems like that's what we do around here."
**Companions:**
[2026-07-13_stylo_fork_decomposition_and_divergence_plan.md](./2026-07-13_stylo_fork_decomposition_and_divergence_plan.md)
(the fork stays the full-fat engine; the two plans share their first
deliverable, the consumed-property audit),
[2026-07-13_genet_consumed_css_property_audit.md](./2026-07-13_genet_consumed_css_property_audit.md)
(the landed census and seam correction),
[2026-07-13_cambium_css_lane_audit.md](./2026-07-13_cambium_css_lane_audit.md)
(the first-lane decision and 40-property catalog contract),
[2026-07-02_gosub_lessons.md](./2026-07-02_gosub_lessons.md) (the
existing gosub harvest).

## Why two engines

It is the established posture, not an exception: boa + nova on the JS
lane, scrying/grafting/weld behind `SurfaceEngine`, netrender's
pluggable backends, three engine kinds in inker. The pairing here:

- **genet-stylo (the fork)** stays the web-correct engine for the
  fullweb lane — 450 longhands, spec-hardened, opportunistically merged
  from upstream.
- **Livery** is grown, not ported: a Genet-native cascade sized
  to a chosen lane, owned end to end, without Gecko residue, Python, or the
  75.6k-LOC monolith on the critical path. The full current Genet path
  consumes 126 longhands through 16 Stylo style structs; a smaller number
  requires a lane-specific boundary rather than a whole-crate engine swap.

The candidate first consumers made the split concrete: Cambium host UI,
smolweb documents, and card-sized content are small
DOMs with small property needs that currently pay for the whole stylo
build and runtime. Fullweb pages keep stylo. Per-document engine choice
is the same shape as the browser multiplexer.

The landed E0b audit chose Cambium's toolkit-owned structural CSS. Its original
22-longhand structural seed grew to 40 when the first real component-catalog
theme was added. Engine-native Nematic does not use CSS, and cards are an
application corpus for the Cambium lane.

## Prior art, engine by engine

**Stylo / Firefox** — the incumbent teaches the big structural lessons:
the property system is the engine's bulk (a declarative property DB +
codegen producing ~98k lines; everything else — matching, cascade,
invalidation — is comparatively small); the rule tree shares cascade
ancestry between elements; bloom-filtered ancestor hashes make selector
matching scale; style structs partition ComputedValues so unchanged
groups are shared, not copied. Take the *shapes*; port no code (license
lane below).

**Chrome / Blink** — independently converged on the same codegen bet:
`css_properties.json5` + Python generators emit the C++ property
classes. Its distinct lessons: ComputedStyle splits rarely-set fields
into copy-on-write "rare data" groups, and the MatchedPropertiesCache
memoizes cascade outputs for elements with identical matched rules —
the cheaper cousin of style sharing, worth considering before any
sharing cache.

**WebKit** — same story (`CSSProperties.json` + generation). The
three majors agreeing is the strongest possible endorsement that a new
engine starts with a property database and a generator, never
handwritten property code.

**Ladybird / LibWeb** — the from-scratch existence proof, and the
closest philosophical match to the knockout doctrine: it generates from
`Properties.json`, started with a small property set, and grew
property-by-property with WPT as the ratchet, favoring spec-literal
readability first and optimizing later. That is exactly the growth
model here — and Genet already owns the ratchet (the WPT baselines +
reftest harness the current `genet-layout` runs today).

**Gosub** — the warning label, already harvested in the gosub lessons
doc: its parsers "exist to be independent, not better." Independence is
not a reason. Livery is justified by *fit* (lean property set,
Genet-shaped Device/media integration, owned growth curve), or not at
all. Gosub's transferable piece (gosub_lattice table layout) belongs to
the current `genet-layout` regardless of engine.

**Blitz** — not an alternative engine; the other Rust *consumer* of
stylo (with taffy/parley/vello, the same decomposition mere uses). Its
role here is the null hypothesis: stylo-as-dependency is viable and is
what Genet does today. Livery must beat it on the measures
that matter to us — cold build (30m35s whole-tree baseline), dev
iteration, binary size, and API fit — or it doesn't deserve to exist.

## The head start: the substrate is already shared crates

A CSS engine is five things, and two of them are free:

1. **Tokenizer/parser** — `cssparser` is a standalone crate (stylo
   itself consumes it). Shared.
2. **Selector matching** — `selectors` is a standalone crate, already a
   fork-family member Genet builds. Shared, including specificity and
   the invalidation-relevant hashes.
3. **Property system** — the property DB + codegen + specified/computed
   value types. **This is the engine.** Built new, in Rust: a TOML
   property database (upstream stylo itself moved to TOML — same shape)
   and a Rust generator (build.rs or committed-output xtask, no
   Python). This is where "port the codegen off mako" lands: not as a
   stylo retrofit (rejected in the fork plan) but as Livery's
   foundation, sized to Genet's audit instead of Gecko's 450.
4. **The cascade** — origin/layer ordering, specificity application,
   inheritance, initial values, importance. Small once the property
   system exists; spec-literal first, Ladybird-style.
5. **Media evaluation** — Genet already owns opinions here (the fork's
   media-feature work, the `MediaEnvironment` consolidation); Livery
   engine gets a Device shaped for Genet hosts from day one.

Value types are the honest hard part: even a small lane subset needs lengths,
percentages, calc(), colors, and round-trip serialization. Stylo's
values/ is 47.8k LOC for the full set; the lean subset is thousands of
lines, not hundreds. The win is not avoiding CSS — it is avoiding the
~90% Genet doesn't consume, plus owning the growth curve.

## The seam: how Genet hosts two engines

The landed audit found 126 consumed longhands across 16 Stylo style structs:
59 through `stylo_taffy`, 73 through direct Genet reads, 30 through
`getComputedStyle`, and 13 animation/transition controls, with overlaps.
`genet-layout` also has 257 `style::` references across 24 source files.
The proposed `ComputedValues` clone with 33 accessors therefore cannot swap
the full crate.

Computed styles remain hot-path data, so a trait-object call per property read
is still the wrong seam. The viable type-level seam is a neutral
`layout-style-api` owned by the selected lane, with concrete computed-value
types behind it. That contract must precede the second engine rather than
graduate later.

There is also a selection mismatch to resolve: a Cargo feature chooses one
engine for the whole build, while the intended product behavior chooses an
engine per document. Runtime choice requires both engines behind a shared
document-facing boundary, or separate layout implementations selected by the
document multiplexer.

The consumed-property audit (Track 2a of the fork plan) was the shared first
deliverable of both plans. It supplies the fork's hard keep-set and bounds the
full swap at 126 longhands. The Cambium audit then supplied the first lean-engine
database: a 22-longhand structural seed with initial values, grammars,
inheritance, animation classes, and specification sources. The component-catalog
theme adds 18 longhands for a guarded 40-property contract.

## Licensing and home

**Clean-room, MIT/Apache-2.0, edition 2024** (founding convention). No
stylo/Gecko code may be ported in — MPL infects files it derives from.
Reading Ladybird (BSD-2) and gosub (MIT) is fine; *depending on*
cssparser/selectors (MPL-2.0) is fine (file-level copyleft does not
reach dependents); copying stylo source is not. This is also the
practical reason the engine is a new codebase rather than a stripped
fork: a stripped stylo is forever MPL and forever merge-taxed;
Livery is neither.

**Home:** `components/livery` in Genet, for the tight edit loop with the current
`genet-layout`, with
the extraction-to-sibling-repo option held open once it stabilizes —
both patterns are established. The crates.io names `genet` and `genet-stylo`
were claimed by this project on 2026-07-13. Genet is the engine formerly called
Serval. Livery's `livery` and `genet-livery` names were claimed the same day;
`livery` is the standalone engine and `genet-livery` is its Genet integration.

## Stages, each with a receipt

- **E0a - current-consumer audit: landed.** The checked-in audit establishes
  the 126-longhand full path, the 16 incumbent structs, and the wider API seam.
- **E0b - lane choice + seed database: landed.** Cambium structural UI is the
  first lane. Its generated declarations plus Genet's baseline UA sheet produce
  a 22-longhand structural seed. The real Cambium component-catalog theme adds
  18 longhands for a 40-property contract. The checked-in `properties.toml`
  records name, concrete value family, inheritance, initial value, grammar,
  seed values, animation class, and specification source. An executable guard
  expands the theme's
  shorthands and rejects declarations absent from the database.
- **E1 — codegen + values: landed.** Livery's Rust generator emits property and
  shorthand enums, 20 concrete value families, and a 40-field
  `ComputedValues` with generated CSS initial values and inheritance behavior.
  The seed layer covers px/em/rem lengths, percentages, linear `calc()`,
  colors through cssparser's hardened color tables, and the lane's keyword and
  numeric types. Receipt: generated code compiles standalone; ten catalog,
  initial/inheritance, rejection, and CSS round-trip tests are green.
- **E2 — cascade + media: landed.** Livery parses declarations and the lane's
  margin, padding, border, and white-space shorthands; matches structural,
  attribute, state, and combinator selectors through `selectors`; and resolves
  origin, layer, importance, specificity, source order, CSS-wide keywords, and
  inheritance into its generated `ComputedValues`. Media-query lists evaluate
  viewport, input, accessibility, display, scripting, and color features on a
  Genet-shaped `Device`. Receipt: the hand-built style-resolution corpus is
  green, including an integrated selector + media + cascade rule.
- **E3 - lane integration: partial.** The `genet-livery` integration crate now
  adapts any `LayoutDom` to Livery selectors, combines clean-room UA rules,
  author sheets, inline declarations, media, and host interaction state into a
  concrete style plane, and lowers the ratcheted geometry, flex, and grid
  subset into standalone
  Taffy fragments. The audited Cambium catalog resolves and lays out through
  this path. A cross-engine receipt agrees on explicit/available widths and
  explicit heights for the catalog's structural boxes. The lane now emits box
  backgrounds and physical borders through the neutral `PaintList` API, with
  Genet engine identity and caller-owned generation IDs. Consecutive text and
  inline-element children shape in one Parley context, sharing line breaks,
  baselines, per-span style, and collapsed whitespace. `LiveryDocument` retains
  the Parley contexts and stable `Arc` font resources, and reuses a complete
  frame at an unchanged viewport. The `genet-documents` `livery` feature
  registers this owner under the opt-in `genet.livery` static rung and lowers
  its PaintList through the shared netrender translator. The default route is
  unchanged. Parley's positioned output now gives wrapped inline elements one
  paint fragment per line and places `inline-block` children atomically in the
  shared line. `cargo tree` proves the `genet-livery` normal/build graph
  contains neither `genet-layout` nor Stylo. Livery now routes a bounded
  viewport scroll offset through the retained PaintList, performs
  pointer-events-aware hit testing, retains link rectangles, handles fragment
  navigation, and feeds focus/focus-within state back into the cascade.
  Rounded backgrounds are clipped through the neutral paint stack. Two-stop
  linear-gradient backgrounds paint as an ordered neutral layer over the color
  fill under that clip. The retained session also has a host-driven opacity
  clock and bounded CSS opacity/background-color transitions, with an
  intermediate-frame receipt proving `transition: all` updates both paint
  properties from the same clock.
  Raster `data:` background URLs now lower into the neutral image side-table.
  Host-resolved local image bytes now feed the same neutral image side-table,
  and the WPT command surface accepts `--renderer livery`, routing bounded
  inline and local linked stylesheet cases plus local image URLs and raster
  `data:` backgrounds through the clean-room producer. Stylo remains the
  default. Bounded opacity-only `@keyframes` declarations and linear, ease,
  ease-in, ease-out, and ease-in-out timing functions run on the retained
  clock. Bounded intrinsic tiling and position/repeat modes now have paint-list
  receipts. Remaining: remote resource loading, replaced-element image layout,
  broader transition-property lists and interpolation, full reftest parity, and
  an apples-to-apples
  cold-build delta against the 30m35s baseline.
- **E4 — first production lane.** One real lane (host chrome or
  smolweb) ships on Livery by default. Receipt: the lane's
  existing suites green + capture receipts.
- **E5+ — grow by ratchet.** New longhands enter via properties.toml +
  WPT/reftest additions, Ladybird-style. Fullweb stays stylo until the
  audit says otherwise.

### 2026-07-15 capability-ratchet receipt

The executable growth pass expands Livery from the original 40-property
catalog plus the earlier opacity/transform rows and bounded transition
controls to 87 properties. It adds
  `right`/`bottom`, min/max sizing, `box-sizing`, `aspect-ratio`, the four
physical corner-radius longhands and `border-radius`, visibility and
pointer-events state, text alignment and spacing, text-decoration color,
  box-shadow, the two-stop background-image gradient subset, flexbox, and a bounded grid track/placement family. Taffy consumes
the geometry, flex, and grid values; Parley consumes alignment and spacing; the
neutral border and shadow primitives carry radii and shadows; hidden boxes
retain layout space while suppressing paint. The receipt is the Livery and
genet-livery test suites. The retained interaction receipt now covers viewport
scrolling, link rectangles, pointer-events hit testing, fragment navigation,
and focus routing through genet-documents. Rounded background clipping is
covered by the paint-list receipt. The gradient receipt and opacity clock are
  covered by the paint-list, cascade, and interaction suites. The keyframe
  parser and retained opacity animation receipt cover named timing functions.
  Resource-backed images, broader transition-property lists and interpolation,
  full reftest parity, and the cold-build comparison remain explicit next gates.
  The image receipt covers raster
  `data:` URLs and host-resolved local bytes; remote loading and replaced
  element layout still belong to the host fetch/cache and intrinsic-sizing
  seams.

## The destination, named

*(Amended 2026-07-14.)* Livery's document profile grinds toward full browser
conformance: every longhand, on the WPT ratchet, Ladybird-style. That is a
destination, not a schedule. The stages above are unchanged, fullweb ships on
genet-stylo, and the switch happens per lane when receipts beat the incumbent
rather than by decree. Naming the destination costs four decisions made now so
that day stays reachable:

- **ComputedValues plans a grouping layer.** A flat 40-field struct is right
  for the seed and wrong at 450; all three majors partition computed style
  into shared groups with copy-on-write rare data. The generator reserves the
  grouping seam now, even while every profile emits a single group.
- **Custom properties get their slot early.** `var()` substitution forces an
  unparsed-value deferral through the parsing path, and retrofitting that slot
  is the classic pain. The declaration representation carries the slot before
  any lane implements it; chisel theming remains the implementation trigger.
- **The schema is written for 450, populated with 40.** `properties.toml`
  stays shaped like the full property space (the same TOML shape upstream
  stylo migrated to); lane-specific shortcuts live in profiles, never in the
  schema.
- **Spec-illegal extensions get a namespace.** Host-lane inventions (spring
  timing functions, field-bound values, host cascade origins) live behind a
  host profile or a prefixed lane, so the document profile stays WPT-clean
  from day one. The two identities, host GUI engine and future fullweb engine,
  never share a namespace and so never conflict.

## Non-goals, named

- Not a near-term stylo replacement on the fullweb lane: the fork plan
  stands, including realignment onto upstream releases. The destination
  section above names the long grind; genet-stylo carries fullweb until
  the receipts say otherwise.
- No parallel traversal, no style sharing cache, no rule tree in the
  first cut — small-DOM-first; add sharing/memoization
  (MatchedPropertiesCache shape first) only when a measured consumer
  needs it.
- No independent selector or parser implementations (gosub's mistake) —
  cssparser/selectors are shared substrate.
- No custom properties in E0–E3; staged in when a consumer lane needs
  them (chisel theming is the likely trigger).
