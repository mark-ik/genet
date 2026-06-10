# CSS rendering conformance plan (Lane C, reftest axis)

Status: **plan (2026-05-31).** Spun out of the [two-lanes doc](./2026-05-29_serval_two_lanes.md)'s
Lane C completeness axes (Layout / Paint / Text / Parsing). The DOM-API axis is
mature (html/dom 39k+ testharness subtests); this plan targets the axis with hard
evidence of being far behind: **CSS rendering, measured by reftest pass rate.**

## The signal that justifies this plan

Reftest pass rates measured 2026-05-31 (`serval-wpt reftest <subset>`, GPU):

| subset | pass / total | note |
| --- | --- | --- |
| `css/CSS2/floats` | 7 / 197 | mixed `.html` + `.xht` |
| `css/CSS2/normal-flow` | 1 / 1045 | **961 of 1045 are `.xht`** |
| `css/css-backgrounds` | 15 / 1326 | **1222 `.html`, only 104 `.xht`** |

Set against: the `html_to_pixels_e2e` suite proves text glyphs, borders, box-shadow
(hard + blur), backgrounds, background-image tiling, `<img>`, floats, and
relative/absolute positioning **all rasterize correctly in isolation**. So this is
not "rendering is broken." It is a **systematic discrepancy** between "renders
correctly in a unit test" and "matches the reference pixel-for-pixel across the
corpus." That gap-shape is the highest-leverage kind — one systematic fix can move
thousands of tests, the same pattern the reflected-attribute lever followed on the
DOM side (4936 → 35k html/dom subtests from one layer).

**Correction this plan records:** the [holistic audit](./2026-05-29_serval_holistic_audit.md)
named "text-to-pixels" as the headline rendering gap. That was stale — text
rasterizes and passes `html_to_pixels_text_rasterizes_glyphs`. The real headline
gap is the reftest pass rate. The two-lanes Paint axis is corrected to match.

## Two systematic causes, already identified

The near-uniform failure splits into two distinct, findable causes — confirmed by
breaking the subsets down by file type:

1. **XHTML (`.xht`) is unparsed.** `normal-flow` is 961/1045 `.xht`; serval's
   parser is HTML-only, so those documents never build a correct tree and the
   render is garbage. This inflates the apparent rendering gap — most of
   normal-flow's 1/1045 is a *parsing* miss, not a render bug. **Lever 1 below.**
2. **A real systematic HTML-reftest diff.** `css-backgrounds` is 1222 `.html`
   yet 15/1326 — and backgrounds demonstrably rasterize (e2e). So something
   uniform is wrong on the parseable-HTML path. Candidates (the diagnosis is the
   plan's first real work item): UA-stylesheet gaps (default margins / font-size
   shifting every box vs the reference), the reftest comparison itself
   (anti-aliasing / tolerance / no fuzzy match), font-metric mismatch vs the
   reference's rendering, or specific unimplemented properties. **Lever 2.**

## Lever 1 — XHTML via `xml5ever` (the cheap, large unlock)

Mark's steer (2026-05-31): XHTML is in scope. The research says it is mostly
**wiring**, because the pieces already exist:

- **`xml5ever` is already a workspace dependency** (`Cargo.toml`, `0.39`), unused.
- It is a sibling of `html5ever` over **one `markup5ever` interface** — verified:
  xml5ever has no own interface module; its `TreeSink` resolves (via
  `pub use markup5ever::*`) to the **same `markup5ever::TreeSink`** that
  `html5ever` and serval-static-dom's `StaticTreeSink` use.
- So `StaticTreeSink` is **reusable as-is**. The HTML path is
  `html5ever::parse_document(StaticTreeSink::new(), Default::default()).one(input)`;
  the XHTML path is
  `xml5ever::driver::parse_document(StaticTreeSink::new(), XmlParseOpts::default()).one(input)`
  (`XmlParseOpts: Default`). Same sink, same `StaticDocument` output.

Work:

- Add `xml5ever` to serval-static-dom's deps; add `StaticDocument::parse_xml(input)`
  (or a `parse_with(format)` that branches). The `TendrilSink::one` driver shape
  is identical to the HTML path.
- Route by file type / content type in the consumers: `serval-wpt` chooses
  `parse_xml` for `.xht`/`.xhtml`; the live host would branch on
  `application/xhtml+xml`. Un-skip `.xhtml`/`.xht` in the runner (currently a
  deliberate skip).
- Verify the sink's XML-relevant methods behave: `create_pi` (already present),
  namespaced element/attribute names (XML carries explicit prefixes), no quirks
  mode. The sink already has `create_comment` / `create_pi` / doctype, so the
  gap is likely small.

Honest caveats:

- XML is **draconian**: a well-formedness error aborts the parse (vs HTML's
  recover-everything). For the WPT `.xht` corpus that is correct. For the open
  web it means a malformed XHTML page renders nothing — spec-accurate, worth
  knowing.
- Expected win: the CSS2 `.xht` corpus becomes *renderable* (961/1045 of
  normal-flow stops being a parse miss). Whether those then *pass* depends on
  Lever 2 — XHTML unblocks them to be scored, it does not by itself make them
  match.

Done condition: `.xht` documents parse to a correct `StaticDocument` (a smoke
test over a known `.xht`), and the runner runs rather than skips them. Measure
the normal-flow reftest delta.

**Status: Lever 1 done (2026-05-31).** `StaticDocument::parse_xml` (xml5ever over
the shared `StaticTreeSink`, zero sink changes — it compiled first try) +
`parse_auto` (content sniff for path-less callers). The runner routes by file
extension (`is_xml_path`), the reliable signal — an earlier content-sniff
(`contains("xhtml")`) misrouted HTML files that merely mention xhtml and was
replaced. Result: **all 1045 normal-flow files now parse + render with 0
errored/crashed** (were ~unparseable HTML soup before). Pass count is flat
(normal-flow 1→0, floats 7→6) and that is the *expected, honest* outcome: making
`.xht` renderable converts "skipped/garbage" into "scored", and they fail the
pixel match for the **same systematic reason the HTML reftests do (Lever 2)** —
XHTML unblocks them to be measured, it does not by itself make them match. The
floats dip also removed pre-wiring *false* passes (a `.xht` test + `.xht` ref
both rendered as similar HTML-soup garbage → spuriously matched; now they render
as correct, genuinely-compared XHTML). Net: the corpus became *measurable*, which
is the prerequisite Lever 2 needs. A `parses_xhtml_via_xml5ever` unit test guards
the parse path on both namespace and text round-trip.

## Lever 2 — the systematic HTML-reftest diff (the diagnosis-first lever)

This is research-first: **diagnose before building.** The seed test is
`css/css-backgrounds/background-color-body-propagation-001.html` —
`body { background: green; margin: 0 }` should propagate to the viewport, with a
`<p>` of text, compared to a ref. It exercises three systematic things at once:
UA default styling, body→viewport background propagation (a real CSS special-case),
and text. It fails today.

First work item: make the runner able to **say *how* a reftest differs** — today
it only reports pass/fail. Add a diff dump (e.g. write test/ref/diff PNGs, or
report max per-channel delta + differing-pixel count + bounding box) so the cause
is observable, not guessed. Then triage a handful of simple HTML reftests into
buckets:

- **comparison/tolerance** — pixels are nearly right but anti-aliasing or
  sub-pixel rounding fails exact compare → the fix is fuzzy-match plumbing in the
  runner (WPT `<meta name=fuzzy>` already parsed for some; widen).
- **UA stylesheet** — default margins / `font-size` / block defaults differ, so
  every box is shifted → the fix is completing serval's UA default stylesheet.
- **specific feature** — body-background propagation, a missing property → a
  real engine feature, scheduled per-feature.
- **font metrics** — glyph advances/baseline differ from the reference renderer
  → hardest, may stay fuzzy-tolerated.

The triage *is* the deliverable of this lever's first pass: it converts "15/1326,
cause unknown" into a ranked list of systematic fixes, each with an estimated
reftest count, exactly as the WPT-gap-analysis workflow did for the DOM side.

**Status: diagnosis done (2026-05-31), and it overturned the hypotheses.** The
runner now reports diff shape: `diff_stats` (same-dims / differing-pixel count /
max per-channel delta) + `diff_label` bucketing each FAIL (`dims` / `whole` /
`aa` / `local` / `equal?`), with a per-run `fail buckets:` tally on the summary
and `diff=N% maxδ=M` on each `-v` FAIL line. (Care taken: `images_match` keeps its
exact WPT fuzzy semantics — `diff_stats` is a separate diagnostic pass; an initial
refactor that merged them regressed fuzzy matching 15→9 and was reverted.)

**The css-backgrounds verdict: `local=568, whole=6`.** Not the systematic causes
guessed above — it is **neither** UA-stylesheet shift (would be `whole`/`dims`)
**nor** anti-aliasing (would be `aa`). 568 of 574 failures are *localized*: 3-7%
of pixels differ at maxδ=255 (a small, maximally-wrong region; the rest of the
page matches). These are **specific unimplemented paint features**, named by the
failing tests: `background-size` scaling (`background-334`: `100% auto`),
`background-attachment: fixed`, `border-radius` background clipping, `dotted`/
non-solid border styles, `background-clip`. So Lever 2's real work is a **ranked
per-feature paint list**, not one systematic fix — the opposite of the DOM
reflection lever's shape, and worth knowing before sinking effort into a
UA-stylesheet or tolerance pass that the data says won't move the number.

**Next:** rank the `local` features by failing-test count (group the `-v` output
by test-name stem), implement the top paint features (likely `background-size`,
border styles, `border-radius` clipping), re-measure. The `whole=6` minority is
the separate small systematic bucket to glance at (likely the body→viewport
propagation seed test among them).

**Status: Lever 2 first pass done (2026-05-31) — and the `local` bucket was
masking three systematic bugs the per-feature framing missed.** Ranking the
`local` failures put `background-size` at #1 (247 of 568), so it was implemented
first (`paint_emit.rs`: `bg_tile_style_of` reads `background-size`/`-position`/
`-repeat` from the cascade, `resolve_bg_tile` computes the concrete tile
geometry — cover/contain/explicit/auto with aspect preservation — and the emit
tiles or single-paints per axis, clipped to the border box; mirrors Servo's
`display_list::background`). A unit test (`background_size_percent_scales_emitted_tile`)
confirms `50%` of a 100px box emits a 50×50 tile.

But re-measuring showed **zero lift** — byte-identical to baseline. Runtime pixel
dumps (a new `serval-wpt dump <subset>` subcommand writing test/ref PNGs to
`.cargo-check-logs/dump/`) revealed why: background-size was never the blocker.
Three systematic bugs were, found only by *looking at the pixels* (the
diff-bucket statistics alone pointed at the wrong cause):

1. **Head content painted as visible text.** The UA stylesheet had no
   `display:none` for `head`/`title`/`style`/`meta`/`link`/`script`, so every
   test's `<title>` and inline `<style>` CSS source rendered as page text. Since
   test and ref `<head>`s usually differ, this alone produced a `local` diff on
   most of the corpus. Fixed in `ua_defaults.rs` with the WHATWG metadata rule.
2. **CSS `url()` never resolved.** `cascade::parse_stylesheet` hardcoded the
   stylesheet base URL to `about:internal-stylesheet`, so Stylo could not resolve
   a relative `url(support/x.png)` — `SpecifiedUrl::url()` returned `None`, the
   background image never decoded, and **no CSS background image painted anywhere
   in the corpus**. Background-size had nothing to act on. Fixed by threading the
   document's `file://` base URL through `run_cascade` (signature change, all
   callers updated; internal restyle paths pass `None` as a documented follow-up)
   → `make_url_data` → `parse_stylesheet` + `parse_inline_styles`, and teaching
   `ResourceResolver::resolve` to map `file://` URLs (now produced by Stylo) back
   to local paths plus a `ResourceResolver::base_url()` helper.
3. **Per-render image-key collision crashed the run.** Once backgrounds actually
   decoded, the full subset *regressed to 7 passing with ~580 crashes*:
   `paint_list_render::register_images` assigned netrender scene image keys from a
   per-render index (`i + 1`), but the vello rasterizer caches `key → bytes` for
   its whole lifetime and `debug_assert`s identical bytes on key reuse. Every
   render's first image collided on key `1` with different bytes → panic while
   holding the rasterizer lock → poisoned mutex → all subsequent tests crash.
   Fixed in `register_images` by deriving the scene key from the producer's
   (now globally-unique) `ImageResource.key` instead of a per-render counter.

Net effect of the three fixes (background-size rides on top): **css-backgrounds
15 → 95**, normal-flow 1 → 8, floats flat (its tests are mostly image-free float
geometry — a different axis). The lesson recorded for the next lever: **dump and
look at the actual pixels before trusting the diff-bucket label** — `local` was
real but it was the *symptom* of UA/url()/key bugs, not the per-feature paint
gaps the statistics implied.

**Status: `background-origin` + `background-clip` box keywords done (2026-06-01).**
`BgTileStyle` now carries `origin`/`clip` (`BgBox` = border/padding/content);
the emit resolves the tile geometry against the **origin box** (CSS default
padding-box — a correctness fix; the old code used the border box) and clips the
paint to the **clip box** (default border-box), computing both from the
fragment's `l.border` / `l.padding` insets. `background-clip` subdir 0 → 13;
`background-origin` renders correctly (a content-box unit test
`background_origin_content_box_insets_tile` locks the inset math at (20,20) for a
10px-border + 10px-padding box) but most of its WPT refs are hand-built fixed
swatches that demand pixel-exact layout serval doesn't reproduce yet, so they
stay red. `background-clip: text` is **not** modeled (falls back to border-box) —
it's text-shaped clipping, deferred.

**Non-determinism diagnosed and fixed (2026-06-01) — and it was masking a large
systematic under-count.** The full-subset count wobbled across identical runs
(94/95/96 for css-backgrounds). Root cause, pinned by rendering one flipping test
in isolation 3× (it still flipped) and byte-comparing two renders of identical
input (`cmp` → differ, `maxδ=1`): **vello rasterization is not bit-exact
run-to-run** — anti-aliased edge pixels vary by ≤1/255 on a sub-1% sliver. It is
*not* fuzzing (matching is deterministic) and *not* cross-test state pollution
(isolated renders flip too). Zero-tolerance exact-match scoring therefore flipped
borderline tests **and** rejected a large population of visually-correct renders
whose only diff was AA jitter.

Fix: a **GPU-jitter floor** in the reftest comparison (`FUZZ_FLOOR_DIFF=1`,
`FUZZ_FLOOR_PIXELS=0.5%` of the render) applied as a lower bound on every
match — a test's own `<meta name=fuzzy>` still widens it. This absorbs exactly the
measured jitter and nothing near a real paint bug (verified: an unimplemented
feature like `border-image` still fails `[local]` at δ=255). Result: counts are
now **deterministic** (3/3 identical runs) *and* substantially higher, because the
floor stopped penalizing correct-but-AA-jittered renders:

| subset | before floor (jittery) | after floor (stable) |
| --- | --- | --- |
| css/css-backgrounds | ~95 ±2 | **152** |
| css/CSS2/normal-flow | 8 | **174** |
| css/CSS2/floats | 7 | **15** |

normal-flow 8 → 174 is the tell: that subset is layout-geometry tests serval was
positioning correctly all along, failing only on exact-match AA. The floor is the
spec-faithful behavior — real WPT harnesses tolerate GPU fuzz for this exact
reason. (Separately noted: `diff_stats` under-reports tiny diffs as `diff=0%` on
some SVG-background fails — a cosmetic diagnostic-label bug, not a match bug.)

**Also clarified the `local` ranking:** ~185 of the apparent "background-size"
fails are actually **SVG background images** (`vector/`, `*-svg*`) — the `image`
crate is raster-only, so SVGs never decode. The gap is an SVG **decode/lower**
front-end (parse the SVG document to paths, e.g. `usvg`, then feed the existing
vello rasterizer), not the rasterizer itself (vello already fills Bézier paths).
A separate capability, not a background-size gap; set aside as its own axis.

**Status: gradient-series regression found + root/canvas propagation landed
(2026-06-09).** A re-measure after the 2026-06-07 serval-layout paint series (the
linear/radial/conic gradient emitters, list markers, the text-decoration trio,
letter/word-spacing) showed css-backgrounds had *regressed*. On this machine the
pre-series baseline is **147** (the doc's earlier `152` was a different GPU; the
number is GPU-dependent, so the regression was re-measured against a fresh local
baseline, not the recorded figure), and the series dropped it to **141**. A
`git bisect` pinned the first bad commit to `af5d042` ("emit CSS linear-gradient
backgrounds").

Pixel dumps overturned the obvious read (again the lesson holds: the `whole`
diff-bucket label said "layout/UA shift", the pixels said otherwise). `af5d042`
is the *first commit to paint any `background-image`*, and it paints correctly —
at the element's own box. The six regressed tests are all root-element gradients
with a margin (`html { background: linear-gradient(...); margin: 50px }`), where
CSS Backgrounds-3 §root-background requires the background to **cover the whole
canvas**, not the margin-offset root box. Before `af5d042` the gradient was not
painted at all, so test and reference rendered identically blank and *matched
spuriously*; painting it converted six fake passes into honest fails by exposing
a missing feature. This is the same "rendering makes it measurable, spurious pass
becomes genuine fail" shape Lever 1 recorded for the XHTML floats dip.

Fix: **root/canvas background propagation** (`paint_emit::emit_canvas_background`).
The root element's background (or, when the root is transparent, the body's, per
the HTML body→canvas special case) is painted over the whole viewport before the
content walk, and the source element's own-box background is suppressed.
`display: none` / `display: contents` on the source generates no box and so does
not propagate (the `*-propagation` negative reftests). Paint model: the gradient
layers tile against the root's positioning area (its box, carrying the margin
offset and size) and paint across the whole viewport — with `background-size:
auto` the tile is the root box, repeated to fill the canvas, matching the
§root-background reference fixtures. (This rode on top of the gradient-tiling work
below; before that the canvas simply stretched the source gradient over the
viewport.)

Result: **css-backgrounds 141 → 147** (the full regression healed; `whole` bucket
17 → 11, back to the pre-series level). Five of the six regressed tests pass
(`background-margin-root`, `-transformed-root`, `-will-change-root`,
`background-attachment-margin-root-001`/`-002`) plus `box-shadow-body` as a bonus.
`background-position/background-position-right-in-body` stays red: it is a
body-source case with two layers, `no-repeat`, and `right`-edge
`background-position`, which needs `background-position` / `-size` on gradients
(the next per-feature step). floats / normal-flow / css-images are unchanged (the
canvas pass runs for every document but only emits when a root/body background
exists). Three unit tests lock the propagate / suppress / body-source behavior.

**Measured the subsets the recent work actually targeted.** The three-subset
board above cannot see the 2026-06-07 paint series, because gradients, markers,
and text-decoration land in *other* directories. Measured 2026-06-09 (GPU,
`--tests-root tests/wpt/tests`): `css/css-images` **164 / 727** (gradients; 7
errored, worth a look), `css/css-lists` **123 / 347** (markers), `css/css-text-decor`
**203 / 631** (the decoration trio). These should join the scoreboard so this
class of work stays visible.

## Sequencing

1. **Lever 1 (XHTML wiring)** first — concrete, bounded, the dependency is already
   present, and it is the largest single *renderable-corpus* unlock. Ship it, then
   re-measure normal-flow.
2. **Lever 2 diagnosis** — add reftest diff reporting, triage the simple
   css-backgrounds HTML failures into the four buckets, rank by count.
3. **Lever 2 fixes.** First pass done: head `display:none` + CSS `url()`
   resolution + the image-key crash fix (pixel dumps found these three systematic
   bugs hiding inside the `local` bucket, not the per-feature gaps the diagnosis
   guessed), then `background-origin`/`-clip` box keywords, then the **GPU-jitter
   match floor** — which turned out to be the biggest single lever (it was fuzzy
   *tolerance* that mattered after all, just as a determinism floor rather than
   the per-test author fuzz). Next, the genuine per-feature paint tail now that
   backgrounds render and the corpus is stably measurable: `border-image`,
   non-solid border styles, `border-radius` background clipping, box-shadow
   detail, then gradients / `opacity` / `transform`, and the Layout tail
   (`inline-block`, `table`, overflow, writing-modes). SVG backgrounds are their
   own axis (needs an SVG decode/lower front-end, not a rasterizer — vello
   already fills vector paths).
4. **Gradient emitters + root/canvas propagation done (2026-06-07 / 06-09).**
   Linear / radial / conic gradients emit (landing in `css-images`); list markers
   and the text-decoration trio landed alongside. The gradient series regressed
   css-backgrounds by exposing a missing feature (root/canvas background
   propagation), now fixed. Next per-feature step on the backgrounds axis:
   `background-position` / `-size` on gradients (unblocks the multi-layer /
   `no-repeat` body-source cases like `background-position-right-in-body`).

## Scoreboard

Per-subset reftest pass rate, re-measured after each lever, published like the
testharness per-directory numbers (GPU, `--tests-root tests/wpt/tests`). The
GPU-jitter-floor column was measured on a different machine (`152`); the
2026-06-09 column re-baselines on the current machine (pre-series `147`), so read
the last two columns as same-machine deltas, not the middle column against the
last:

| subset | Lever-1 baseline | after url()/head/key fixes | after GPU-jitter floor | after canvas + gradient tiling (2026-06-09) |
| --- | --- | --- | --- | --- |
| `css/CSS2/floats` | 7 / 197 | 7 / 197 | 15 / 197 | **15 / 197** |
| `css/CSS2/normal-flow` | 1 / 1045 | 8 / 1045 | 174 / 1045 | **174 / 1045** |
| `css/css-backgrounds` | 15 / 1326 | ~95 / 1326 (±2) | 152 / 1326 | **149 / 1325** |

Subsets the 2026-06-07 paint series targeted (first measured 2026-06-09, no prior
baseline recorded — add to the running board going forward):

| subset | 2026-06-09 | lands here |
| --- | --- | --- |
| `css/css-images` | **164 / 713** (0 errored) | gradients |
| `css/css-lists` | **123 / 347** | list markers |
| `css/css-text-decor` | **203 / 631** | underline / overline / line-through / color |

The css-images `7 errored` were `tools/*-template.html` files the runner was
collecting as reftests (their `rel=match` points at a non-existent ref). Fixed
in `serval-wpt collect` by excluding `tools/` and `support/` directories (WPT
does not treat them as tests); 7 errored → 0, file count 727 → 713, pass count
unchanged.

**`background-size` / `-position` on gradients (scoped 2026-06-09).** Smaller
lever than the gradient fail count suggests. Of css-images' 234 fails, 62 involve
a gradient but only a handful (`tiled-radial-gradients`,
`linear-gradient-body-sibling-index`, the css-backgrounds
`background-position-right-in-body`) are genuine `background-size`/`-position`-on-
gradient gaps; in css-backgrounds the grep also catches `background-repeat: round`
/ `space` tests, a *separate* unimplemented feature. The bulk of the 62 gradient
fails are gradient **color interpolation** (`gradient-*-hsl`/`lch`/`oklch`,
`analogous-missing-components`, decreasing-hue) — CSS Color 4 interpolation, a
deeper and higher-count lever than sizing. Pixel-confirmed shape: serval stretches
one gradient ramp over the box where the reference *tiles* it
(`tiled-radial-gradients` shows two ellipses vs serval's one). Honoring
`background-size` therefore needs gradient **tiling**: the renderer fills one
`placement` per gradient ramp, so a tiled layer emits N gradients (the image tile
path is single-emit).

**Built, reverted once, then re-landed correctly net-positive (2026-06-09).**
The first attempt (per-layer `background-size`/`-position`/`-repeat`, a
positioning-area-vs-painting-area split so the canvas tiles the root box across
the viewport, a tile cap with an area-fill fallback, the `gradient_tile_cmd`
per-tile emitter) was correct for `repeat` / `no-repeat` but scored **net −5 to
−7** and was reverted. The losses were not the tiling itself; they were
*unfinished prerequisites* it exposed. A second pass finished them and the
feature landed **+2 with zero regressions** (css-backgrounds 147 → 149,
css-images steady at 164, floats/normal-flow unchanged). What the prerequisites
were:

- **`background-repeat: round` / `space`.** The first pass mis-tiled them as
  plain `repeat` (or fell back to stretch); both diverge from the reference,
  which pre-computes the rounded tile. Implemented properly: `round` rescales the
  tile so a whole number fills the positioning area (e.g. `32px` → `36px` in a
  `72px` box); `space` distributes whole tiles with gaps, the first/last touching
  the area edges, continued at that period across a larger clip box (so a spaced
  gradient repeats behind a transparent border). This recovered
  `background-repeat-round-1c…3` and the `space` tests and *gained*
  `background-size-041` / `-042`.
- **`background-origin`.** The tile must size/position against the origin box
  (default padding-box, but `border-box` / `content-box` per the property), not
  the border box. Hardcoding padding-box regressed `gradient-border-box` /
  `gradient-content-box`; reading `background-origin` per layer fixed both.
- **`background-attachment: fixed`.** A fixed canvas layer is viewport-anchored,
  not root-box-anchored; positioning fixed layers against the painting area (the
  viewport, for canvas propagation) recovered `background-attachment-margin-root-002`.

Two originally-named targets stay red for reasons *outside* the tiling, now
understood: `tiled-radial-gradients` renders correctly under tiling but its
reference is mis-rendered by serval — and the cause is **not** the abs-pos
static-position bug guessed earlier (a unit test confirms two abs-pos siblings
share static `y=0`); it is a discrepancy between the unit layout path and the
full render pipeline (the rendered box lands at `y=74` and the blobs stack),
recorded as a separate render-path follow-up. `background-position-right-in-body`
is a sub-pixel near-miss. Four unit tests lock auto-fill / no-repeat / repeat /
round in `paint_emit.rs`.

All subsets report **0 errored** on the three-subset board (the image-key crash
is gone) and are now **deterministic** (identical re-runs) after the GPU-jitter
floor. css-backgrounds `147` is below the recorded `152` only because of the
machine re-baseline (pre-series `147`): the gradient-series regression that
dropped it to `141` is healed back to the `147` local baseline. The floor was
the single largest lever — not because matching was loosened past correctness
(an unimplemented `border-image` still fails δ=255), but because zero-tolerance
exact-match had been systematically rejecting visually-correct renders whose only
diff was ≤1/255 anti-aliasing jitter; normal-flow 8 → 174 shows how much
correct-but-AA-jittered layout that was hiding. The remaining css-backgrounds
failures are now *genuinely* per-feature paint gaps (border-image, non-solid
border styles, `border-radius` clipping, box-shadow detail) plus the separate
SVG-background axis.

## Non-goals (for now)

- CSS animations/transitions (their own axis; the alphabetically-first
  css-backgrounds failures are `animations/` and are fair fails).
- The full WPT server (remote resources); reftests load files directly.
- HTML serialization round-trips. Parse-in only.

## Relationship to other docs

- [two-lanes](./2026-05-29_serval_two_lanes.md): this is the Lane C Parsing +
  Paint + Layout axes, spun out.
- [blitz float/linebox study](./2026-05-20_blitz_float_linebox_study.md): the
  `inline-block` / anonymous-box approach for the Layout tail.
- [WPT runner plan](./2026-05-26_wpt_runner_plan.md): the reftest harness this
  scores against; Lever 2 extends it with diff reporting.
