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

## Sequencing

1. **Lever 1 (XHTML wiring)** first — concrete, bounded, the dependency is already
   present, and it is the largest single *renderable-corpus* unlock. Ship it, then
   re-measure normal-flow.
2. **Lever 2 diagnosis** — add reftest diff reporting, triage the simple
   css-backgrounds HTML failures into the four buckets, rank by count.
3. **Lever 2 fixes** in ranked order — likely UA-stylesheet completeness and
   fuzzy-tolerance first (systematic, high count), then per-feature CSS gaps
   (`border-radius`, gradients, `opacity`, clipping, `transform`, stacking —
   the Paint axis tail) and the Layout tail (`inline-block`, `table`, overflow,
   writing-modes).

## Scoreboard

Per-subset reftest pass rate, re-measured after each lever, published like the
testharness per-directory numbers. The Lever-1 baseline to beat: floats 7/197,
normal-flow 1/1045, css-backgrounds 15/1326.

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
