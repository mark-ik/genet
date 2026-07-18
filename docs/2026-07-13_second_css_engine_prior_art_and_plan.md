# A second CSS engine: prior art and growth plan

**Date:** 2026-07-13
**Status:** E0a full-path audit, E0b Cambium lane choice/40-property catalog,
E1 values/codegen, E2 style resolution, and the first E3 integration slice are
landed. The catalog has since ratcheted to 88 properties. `genet-livery` now adapts `LayoutDom` into Livery's selector and cascade
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
`transition-property`/`transition-duration` metadata starts opacity,
background-color, text-color, and the four physical border-color and
border-width, border-style, and background-repeat transitions on that same clock, so `transition: all` and explicit
multi-property lists can
paint those changes in one retained tick. Nested scroll containers now route wheel deltas
into retained offsets, chain at their boundary to the viewport, and replay
descendant paint through transforms. Bounded opacity-only `@keyframes` and
named timing functions now run on the retained clock. Host-owned remote image
fetching now feeds the same resource seam. Remaining E3 work is full WPT
reftest parity and interpolation beyond the bounded
opacity/background-color/color/four-side-border-color/border-width/
border-radius/transform/background-position/box-shadow/background-image paths.
A fresh
workspace cold-build receipt is recorded below. The
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
  color-only background, directional border, margin, padding, border, and
  white-space shorthands; matches structural,
  attribute, state, and combinator selectors through `selectors`; and resolves
  origin, layer, importance, specificity, source order, CSS-wide keywords, and
  inheritance into its generated `ComputedValues`. Media-query lists evaluate
  viewport, input, accessibility, display, scripting, and color features on a
  Genet-shaped `Device`. Receipt: the hand-built style-resolution corpus is
  green, including an integrated selector + media + cascade rule.
- **E3 - lane integration: partial.** The `genet-livery` integration crate now
  adapts any `LayoutDom` to Livery selectors, combines clean-room UA rules,
  author sheets, inline declarations, media, and host interaction state into a
  concrete style plane, and lowers the ratcheted geometry, flex, grid, and
  bounded float subset into standalone
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
  clock and bounded CSS opacity/background-color/color/border-top-color/border-bottom-color/border-radius/transform transitions, with
  intermediate-frame receipts proving `transition: all` and the explicit
  two- and three-property lists update their paint properties from the same
  clock.
  Raster `data:` background URLs now lower into the neutral image side-table.
  Host-resolved local and remote image bytes now feed the same neutral image side-table,
  and the WPT command surface accepts `--renderer livery`, routing bounded
  inline and local linked stylesheet cases plus local image URLs and raster
  `data:` backgrounds through the clean-room producer. Stylo remains the
  default. Bounded opacity-only `@keyframes` declarations and linear, ease,
  ease-in, ease-out, and ease-in-out timing functions run on the retained
  clock. Bounded intrinsic tiling and position/repeat modes now have paint-list
  receipts. Remaining: interpolation beyond the bounded
  opacity/background-color/color/four-side-border-color/border-width/
  border-radius/transform/background-position/box-shadow/background-image paths
  and full reftest parity.
  The cold-build delta against the 30m35s baseline is recorded below.
- **E4 — first production lane.** One real lane (host chrome or
  smolweb) ships on Livery by default. Receipt: the lane's
  existing suites green + capture receipts.
- **E5+ — grow by ratchet.** New longhands enter via properties.toml +
  WPT/reftest additions, Ladybird-style. Fullweb stays stylo until the
  audit says otherwise.

### 2026-07-15 capability-ratchet receipt

The executable growth pass expands Livery from the original 40-property
catalog plus the earlier opacity/transform rows and bounded transition
controls to 88 properties. It adds
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
  Additional transition-property lists and interpolation beyond the bounded
  color, four physical border-color, four physical border-width, radius,
  transform, background-position, box-shadow, and background-image lanes, plus
  full reftest parity,
  remain explicit next gates.
  The image receipt covers raster `data:` URLs, host-resolved local and
  remote-looking bytes, intrinsic `<img>` sizing, and aspect-ratio preservation;
  the session now exercises the host fetcher for a remote image while keeping
  URL policy and caching host-owned.

### 2026-07-15 remaining-gate receipt

The bounded explicit `opacity, background-color` and
`opacity, background-color, color` transition lists now have cascade, value
round-trip, and retained mid-frame receipts. A standalone `color` transition
also paints an interpolated Parley text run through the same retained clock.
The standalone `border-top-color` transition likewise paints the neutral top
border through that clock; the fixture uses the supported physical border
longhands rather than the unimplemented `border-top` shorthand. The matching
`border-bottom-color` transition paints the neutral bottom border through the
same clock, with the same physical-longhand fixture boundary.
The matching left and right border-color lanes now parse and round-trip as
bounded transition properties, schedule from the retained clock, and sample
through the existing neutral `DrawBorder` side colors. One `transition: all`
receipt exercises both sides at an intermediate frame and after settlement.
The transition-property value now preserves arbitrary combinations of the
supported property bits, so a shorthand list containing opacity and both side
colors survives cascade merge and round-trip serialization.
The Livery session now resolves a CSS/DOM image URL against the document
address, asks the host `ResourceFetcher` for bytes, and paints the returned
remote image; URL policy and caching remain host-owned.

The focused Livery/genet-livery clippy wall passes with `-D warnings`.
`genet-documents` strict clippy remains blocked by the existing `pelt-core`
`clippy::derivable_impls` error, outside this slice.
The border-side color ratchet keeps the focused receipts green: Livery has
11 cascade and 4 value tests, genet-livery has 14 interaction and 41 paint
tests, and genet-documents has 19 feature-gated tests.

The WPT producer helper tests pass. `css/CSS2/box/ltr-basic.xht`,
`css/css-backgrounds/background-image-001.html`, and
`css/css-backgrounds/background-color-clip.html` each pass through both
`--renderer livery` and the default Stylo route; the `.htm` border-box probe is
skipped by the runner. A first `css/css-backgrounds` probe reports 7 passes,
582 failures, 360 skips, and no runner errors. That is a capability map rather
than a parity receipt: the failures cluster around background features outside
the bounded lane and crash-path coverage.
The focused `css/CSS2/box` subset is 9/9 through Livery; the default Stylo
route is 8/9 because `rtl-linebreak.xht` has a localized reference diff. This
is useful route telemetry, not a claim that the incumbent failure transfers to
the clean-room engine.
The full `genet-wpt` unit wall is 18 passed, 1 failed, and 3 ignored; its sole
failure is the existing WebGL `gl_clear` harness panic (`JsError: not a callable
function`), outside the Livery route. After removing 17.9 GiB of targeted
Livery/document artifacts, `cargo check -p genet-documents --features livery`
completed in 57.1 seconds. That is useful package-graph telemetry, not an
apples-to-apples replacement for the 30m35s whole-workspace cold-build
baseline. A fresh `CARGO_TARGET_DIR=C:\\t\\genet-cold-target cargo check --workspace`
then completed with exit 0 in 5m24s (323.9s). That is about 25m11s faster than
the recorded baseline under the current checkout and source-cache state; it is
useful comparison telemetry rather than a controlled benchmark. Cargo emitted
the existing path-override and unused-code warnings, with no errors.

### 2026-07-17 WPT pipeline-isolation receipt

The WPT renderer now allocates a fresh paint pipeline for each render, retires
it after readback, and remaps each frame's image resources and image commands
into a fresh namespace. The latter closes a real NetRender lifetime seam:
producers restart image keys at one while the long-lived atlas retains entries
after pipeline exit. Before the remap, `line-box-height-002.xht` crashed when a
96x96 image reused a key previously registered as 15x15; it now reaches the
expected localized pixel comparison.

The focused renderer-selection unit test passes. With the rebuilt runner and
the live manifest root `tests/wpt/tests`,
`css/CSS2/linebox` reports 142 passed, 48 failed, 59 skipped, and 0 errored
through Livery. The same isolated harness reports 118 passed, 72 failed, 59
skipped, and 0 errored through Stylo. This turns the previous Livery crash-heavy
directory run into usable parity telemetry while leaving the 48 localized
linebox mismatches open for the next layout/paint slices. Full WPT parity
remains an explicit gate.

Full WPT reftest parity and interpolation beyond the bounded color, four
physical border-color, four physical border-width, radius, transform,
background-position, box-shadow, and background-image lanes, plus
the E4 default production-lane switch, remain open. The standalone Cambium
WebGPU smoke now passes native `cargo check` plus wasm32 `cargo check` and
`cargo build` after local `genet-scripted-dom` and `layout-dom-api` patching;
wasm-bindgen also emits the web package. The browser runtime leg remains
unverified in this environment because no controllable browser service is
available.

### 2026-07-17 inline decoration and shorthand receipt

The Livery database now admits the bounded color-only `background` shorthand
and the four directional border shorthands. Their cascade expansion is covered
by 13 green Livery cascade tests. Parley inline atoms now separate margin-only
advance from paintable padding and border edges, so an inline margin shifts the
following glyph without extending the inline background into the margin; the
41-test genet-livery paint wall remains green.

The six CSS2 inline-formatting probes used for this slice now exercise the
bounded float direction in the reference fixture. Four pass through Livery;
the two margin probes remain localized line-height mismatches. The parser and
inline-atom receipts are landed without treating the remaining two probes as
parity passes.

### 2026-07-17 bounded float-layout receipt

Livery now admits the `float` longhand with `none`, `left`, and `right` values.
The generated value family round-trips through the catalog, and
`genet-livery` maps it into the fork's existing Taffy `float_layout` lane.
The focused inline-formatting probes for left/right borders and padding pass
through the clean-room renderer (4/6); the two margin probes remain localized
because their explicit `line-height: 1em` fixture still differs from the
reference block's normal line-height.

The walk-discovery `css/CSS2/linebox` run now reports 142 passed, 48 failed,
120 skipped, and 0 errored. The remaining failures are the existing line-height,
vertical-align, empty-inline, and inline-box groups plus the two margin probes;
the float reference dependency is no longer an unimplemented-property failure.

### 2026-07-17 retained radius interpolation receipt

The retained transition clock now accepts the bounded `border-radius` shorthand
and interpolates its four physical corner radii. Zero-to-px and same-unit
length/percentage radii interpolate through a typed `Radius` helper; mixed
non-zero units remain discrete until the value ratchet expands them. A retained
mid-frame interaction test observes the neutral `DrawBorder` radius at 10px
between 0px and 20px, then observes the settled 20px value. The focused Livery
value wall has 5 tests, the cascade wall 13, and the genet-livery interaction
wall 15; all pass.

### 2026-07-17 retained transform interpolation receipt

The transition-property ratchet now preserves a `transform` bit alongside the
existing paint properties. Matching transform-function lists interpolate
translations, scales, and rotations; mixed function shapes, units, and `none`
normalization remain discrete until the matrix ratchet. A retained interaction
test observes a 10px/2px translation at 50ms between 0px/0px and 20px/4px,
then observes the settled transform through the neutral coordinate-space
primitive. The focused Livery value wall has 6 tests and the genet-livery
interaction wall 16; both pass with the paint wall.

### 2026-07-17 retained background-position interpolation receipt

The retained transition clock now accepts the bounded two-component
`background-position` lane. Length/percentage components and zero-to-unit
interpolation share the typed `LengthPercentage` helper; mixed non-zero units
and `calc()` expressions remain discrete. A retained interaction test moves a
2x3 host-resolved image from `left top` to `right bottom`, observing the
39px/18.5px midpoint and the settled 78px/37px placement. The focused Livery
value wall has 7 tests, the cascade wall 14, and the genet-livery interaction
wall 17; all pass with the paint wall.

### 2026-07-17 retained box-shadow interpolation receipt

The retained transition clock now accepts the bounded single-layer
`box-shadow` lane. Matching length units, inset mode, and the serializable RGBA
color subset interpolate through the existing neutral `ShadowItem`; `none`,
mixed units, and mode changes remain discrete. A retained interaction test
observes a 10px/2px, 5px-blur, midpoint shadow between the red zero shadow and
the settled 20px/4px, 10px-blur blue shadow. The focused Livery value wall has
8 tests, the cascade wall 15, and the genet-livery interaction wall 18; all
pass with the 41-test paint wall.

### 2026-07-17 retained background-image interpolation receipt

The retained transition clock now accepts the bounded two-stop linear-gradient
`background-image` lane. Matching gradient shapes interpolate each color stop
through the serializable RGBA subset; `none`, URLs, and mixed image shapes stay
discrete until the image-list ratchet. A retained interaction test observes the
red/blue-to-white/black gradient midpoint, then the settled target stops. The
focused Livery value wall has 10 tests, the cascade wall 16, and the
genet-livery interaction wall 20; all pass with the paint wall.

### 2026-07-17 retained border-width interpolation receipt

The retained transition clock now carries the four physical `border-*-width`
longhands. Fixed keyword widths and matching px units interpolate through the
computed line-width family; mixed non-zero units remain discrete. A retained
interaction test observes all four neutral border widths at 6px between 2px and
10px, then observes the settled 10px widths. The focused Livery value wall has
9 tests, and the genet-livery interaction wall 19; both pass with the paint
wall. The unsupported `border-width` shorthand remains outside this lane.

### 2026-07-17 retained border-style transition receipt

The transition-property mask now uses a `u32` bitset, leaving room for the
remaining physical longhands after the first 16 paint properties. The four
physical `border-*-style` values now travel through the retained clock as
discrete transitions: the source style remains through 49ms and the target
style appears at the midpoint. A retained interaction test exercises solid,
dashed, dotted, double, and groove sides together under `transition: all`.
The focused Livery value wall has 11 tests, the cascade wall 17, and the
genet-livery interaction wall 21; all pass.

### 2026-07-17 retained background-repeat transition receipt

The retained clock now carries the bounded `background-repeat` modes already
consumed by image tiling. Repeat modes remain discrete: a 2x3 host-resolved
image stays at one `DrawImage` before 50ms, then switches to repeated tiles at
the midpoint and after settlement. The focused Livery value wall has 12 tests,
the cascade wall 18, and the genet-livery interaction wall 22; all pass.

### 2026-07-17 replaced-image dimension receipt

Livery's replaced-box sizing now consumes positive HTML `width` and `height`
attributes as presentational dimensions. A definite CSS width or height wins;
an omitted dimension keeps the decoded intrinsic ratio. This closes the
reference-image seam used by the CSS2 line-height corpus without changing the
neutral image resource payload. The focused `genet-livery` replaced-image test
and the full package wall pass. Through the rebuilt clean-room WPT runner,
`line-height-006.xht` and `line-height-072.xht` now pass with their 96px and
120px image references; the walk-discovery `css/CSS2/linebox` run is currently
118 passed, 72 failed, 120 skipped, and 0 errored. The remaining failures are
the physical-unit, inline-decoration, empty-inline, vertical-align, and
line-height groups, so the old 142/48 count is retained only as the
pre-attribute-sizing snapshot.

### 2026-07-17 physical absolute-length receipt

Livery now carries the CSS absolute units used by the linebox corpus:
`in`, `cm`, `mm`, `q`, `pt`, and `pc`. The typed values serialize and resolve
through the 96dpi reference-pixel conversion, and the same helper feeds
Taffy geometry, Parley spacing, paint offsets, shadows, transforms, and
computed font metrics. The focused value wall has 13 tests and the full
`genet-livery` wall remains green. Nine physical-unit line-height probes
(`017`, `018`, `028`, `029`, `039`, `040`, `050`, `051`, and `058`) now pass.
The walk-discovery `css/CSS2/linebox` run is 144 passed, 46 failed, 120
skipped, and 0 errored. Font-relative `ex`/`ch`, inline decoration, empty
inline boxes, vertical-align, and the remaining line-height groups stay open.

### 2026-07-17 bounded font shorthand receipt

The Livery cascade now expands the bounded `font` shorthand into its five
consumed longhands: optional style and weight, required size, optional
line-height, and family. The parser keeps CSS-wide reset behavior outside this
slice and rejects malformed or family-less forms. The focused cascade wall has
19 tests, the values wall 13, and the full `genet-livery` package wall remains
green. The rebuilt Livery runner passes `line-height-002`, `004`, `005`, and
`006`; the remaining selected font-heavy line-box cases still expose the open
font-metric, inline-box, and line-height layout seams rather than shorthand
parse errors.

### 2026-07-17 empty-inline line-box receipt

Empty inline elements now contribute a zero-width, non-painted inline atom
whose height is their computed `line-height`. Borders, padding, and margins
continue to use the existing edge atoms, while nested inline content prevents
the synthetic atom from duplicating a real child. The focused `genet-livery`
package wall remains green. `empty-inline-002.xht` now passes through the
rebuilt Livery runner; `empty-inline-003.xht` reaches a zero-percent image
diff but still fails the exact-match threshold, leaving its surrounding line
box and metric reconciliation open.

### 2026-07-17 bounded vertical-align receipt

Livery now carries the CSS `vertical-align` family through the generated
catalog: baseline, sub/super, text-top/text-bottom, middle, top/bottom, and
signed length or percentage offsets. Parley text runs apply the bounded
baseline shifts, while inline-block atoms align against the shaped line
metrics for middle, top, and bottom. The focused cascade wall has 20 tests,
the values wall 13, and the full `genet-livery` package wall remains green.
The focused `vertical-align-088.xht` through `vertical-align-092.xht` probes
now pass. Ahem-heavy vertical-align cases remain open with the broader font
metric and line-box work; the prior walk-discovery receipt remains 144 passed,
46 failed, 120 skipped, and 0 errored.

### 2026-07-17 inline replaced-image line-fragment receipt

Inline `<img>` elements now retain their preliminary replaced fragment while
the shaped line is built, so the block wrapper's stretched width cannot replace
the image's intrinsic or definite dimensions. CSS and HTML dimensions apply
even when intrinsic bytes are unavailable; the decoded aspect ratio fills only
the remaining auto dimension. The focused native wall adds host-resolved
intrinsic and vertical-offset coverage, and the full `genet-livery` package
wall is green with 43 paint tests. The rebuilt Livery runner now passes
`line-height-126.xht` and `line-box-height-002.xht`; `line-height-127.xht` and
`line-height-128.xht` remain Ahem/font-metric cases.

### 2026-07-17 inline replaced-image margin-box receipt

Inline replaced atoms now carry separate content and margin-box geometry. The
content fragment remains the paint target, while Parley receives signed
horizontal and vertical margins for line participation. This lets a negative
bottom margin collapse a zero-height line box without swallowing the image
paint, and preserves the same seam for inline-block replaced content. The
native paint wall is green with 44 tests, including a retained 100px image
regression.

The focused runner now passes `line-height-129.xht` alongside
`line-height-126.xht` and `line-box-height-002.xht`. `line-height-127.xht` and
`line-height-128.xht` remain Ahem/font-metric cases; their failures are still
font-resource limitations rather than margin-box geometry.

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
