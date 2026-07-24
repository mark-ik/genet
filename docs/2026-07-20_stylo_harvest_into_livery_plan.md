# Stylo harvest into Livery: the climb to fullweb

**Date:** 2026-07-20
**Status:** H0, H1, and H2 landed 2026-07-20; H3 and H4 landed
2026-07-22; H5 active (2D matrices, percentage reference boxes,
nested-calc/CSSOM used values, the full viewport-unit family, writing-mode
mapping, recursive comparison math, size containers, and scoped iframe style
worlds landed 2026-07-22 through 2026-07-23); H6 available.
Census grounded against the local fork checkout at H0.
**Decision record:** Mark, 2026-07-18: "even an mpl-2.0 livery is worth more
than servo's stylo to me. the proof is genet itself," and "level up livery to
cover other tiers, up to and including the fullweb, by decomposing and
incorporating what can be used in stylo." This REVERSES the clean-room
licensing posture in
[2026-07-13_second_css_engine_prior_art_and_plan.md](./2026-07-13_second_css_engine_prior_art_and_plan.md)
(see its amended Licensing section) and SUPERSEDES Tracks 1b and 2a of
[2026-07-13_stylo_fork_decomposition_and_divergence_plan.md](./2026-07-13_stylo_fork_decomposition_and_divergence_plan.md).
The value is ownership and decomposability, not license purity.

**Companions:**
[2026-07-13_genet_consumed_css_property_audit.md](./2026-07-13_genet_consumed_css_property_audit.md)
(the 126-longhand consumed set),
the second-engine plan's staged receipts (Livery's current 125-longhand,
CSS2-linebox-178/12 state).

## The quarry

`Code/crates/stylo` is the mark-ik fork checkout, branch `genet-rename`,
at genet-stylo 0.19.1, with servo/stylo wired as `upstream`. The lift base
is the fork line, not upstream: it already carries Mark's commits and
matches what genet-layout consumes. Census (verified 2026-07-20):

- `properties/longhands.toml` 429 entries, `shorthands.toml` 92, plus
  descriptor TOMLs (@font-face, @property, @counter-style,
  view-transitions).
- `custom_properties.rs` 2,919 lines.
- `invalidation/element/` ~6.8k lines across eight files;
  `invalidation/stylesheets.rs` 745.
- `stylesheets/` ~8.5k (rule object model).
- `values/animated/` ~2.7k (transform, color, grid, effects, font).
- `values/` ~48k total; lifted per family, never wholesale.

## Rules of the lift

1. **Subsystem units.** A lift is (types, parse, serialize, behavior,
   tests) for one bounded subsystem, reshaped onto Livery's generated
   catalog types. No file lands without its reshaping pass.
2. **A lift is a fork-and-own event.** Upstream fixes stop flowing to a
   lifted subsystem. Lift what is stable and spec-hardened; leave what
   upstream still churns.
3. **Provenance per file.** Every lifted file keeps its MPL-2.0 header and
   gains a provenance note (source path + fork commit), so upstream diffs
   stay consultable at future realignments.
4. **License mechanics.** Lifted files stay MPL-2.0. Lifted subsystems land
   in provenance-aligned modules or crates; the genuinely clean-room core
   keeps MIT/Apache until mixing gets awkward, then Livery flips to
   MPL-2.0 whole (cambium is the house precedent; the founding convention
   allows MPL for Servo-derived code). Mark-authored fork commits (the
   media tiers) are relicensable freely; they need no MPL treatment.
5. **The ratchet is unchanged.** Every stage lands through the existing
   WPT/reftest walls. Harvest changes the authorship mode of a slice, not
   its receipt.

## Stages, each with a receipt

- **H0 - property database as data: landed 2026-07-20.** The committed
  `components/livery/tools/import-stylo-db` tool (standalone workspace,
  rerun after fork realignments) parses the fork's longhands/shorthands
  TOMLs at rev `b157d92526`, keeps the servo lane (drops
  `engine = "gecko"`: 172+24 entries), and rewrites the marked generated
  section of `properties.toml`. Measured space: 427+92 full, 255+68
  servo-lane, 89+17 implemented, **168 longhands + 49 shorthands
  imported as `[[unimplemented]]` entries** carrying group (the fork
  style struct, the future grouping seam), inheritance (derived from the
  fork's inherited-struct table), animation class, logical flag, aliases,
  and spec URL. Two livery-local names (`background-position`,
  `vertical-align` are upstream shorthands modeled as bounded livery
  longhands) are covered cross-kind, not double-listed. Livery's
  generator validates disjointness and emits
  `UNIMPLEMENTED_LONGHANDS`/`UNIMPLEMENTED_SHORTHANDS` metadata with
  alias-aware lookups; the parser now rejects these names with a
  distinguishable `KnownUnimplemented` diagnostic instead of
  `UnknownProperty`. Receipts: the checked-in
  `components/livery/PROPERTY_SPACE.md` census; the livery wall (56 tests
  incl. the new property-space guard: disjointness, fork-struct
  inheritance agreement, cursor/text-shadow rejected distinguishably,
  word-wrap alias resolution); the genet-livery wall (8+22+44+1+1);
  livery clippy `-D warnings`. Descriptor TOMLs are enumerated in the
  census as out-of-scope; they enter with their subsystems (H1, H5).
- **H1 - custom properties: landed 2026-07-20.** `livery::custom` carries
  the substitution walker, fallback handling, and cycle-scoped
  invalidation, following the fork's `custom_properties.rs` shapes
  (on-demand resolution with an explicit visiting stack, member-wise
  cycle poisoning so upstream referers recover through fallbacks,
  expansion budget) reshaped onto livery's string token layer. The parser
  captures case-sensitive `--name` declarations (CSS-wide keywords
  included) and defers any var()-bearing declaration as a pending value;
  a var()-bearing shorthand stores one pending copy per expanded
  longhand, the fork's WithVariables shape, re-expanded after
  substitution. `cascade_with_custom` resolves the element map (parent
  map inherited wholesale, winners chosen with the same priority rules as
  longhands) and treats substitution or reparse failure as
  invalid-at-computed-value-time, which behaves as `unset`. genet-livery
  threads the map parent-to-child through `resolve_styles` and exposes it
  per element on the style plane. Receipt: a 15-test corpus (substitution,
  fallbacks, nesting, case sensitivity, member-wise cycles, inheritance
  with initial/unset, important ordering, shorthand expansion, empty
  values, quote-aware detection) plus both full walls green. The WPT
  custom-properties directory is testharness/getComputedStyle-driven, so
  it becomes runnable receipt-wise at H3/H4 (the scripted tier), not on
  the static lane; `@property` registered types enter there too.
- **H2 - general interpolation: landed 2026-07-20.** livery's values
  layer now has an `Interpolate` trait registry: every generated
  `PropertyValue` family must declare its interpolation (real math or the
  discrete midpoint default), so adding a family without deciding is a
  compile error. The generator emits `PropertyValue::interpolate`
  (same-family dispatch, discrete otherwise), `ComputedValues::get`
  (tagged reads, the getComputedStyle seam), and
  `TransitionProperty::includes_property` + `TRANSITIONABLE` generalize
  the hand-flag surface. genet-livery's retained clock is re-expressed:
  the 21 per-property transition structs and their 21x4 schedule/find/
  apply/finish lanes collapse into one `PropertyTransition` vector driven
  through the generated dispatch — document.rs fell from 2,808 to ~900
  lines. `animate_opacity`, `pump`, and `settled` keep their public
  contracts. Receipt: the full 2026-07-17 interaction/paint receipts stay
  green through the generic machinery (149 tests across livery +
  genet-livery, zero failures; genet-documents' 21 livery-feature tests
  green; livery clippy `-D warnings` clean — genet-livery has two
  pre-existing too-many-arguments clippy errors in untouched
  paint.rs/text.rs, outside this slice). A new animatable property now
  costs an `Interpolate` impl and a `TRANSITIONABLE` entry, not a
  hand-built lane. The fork's transition *state machine*
  (interrupted-transition reversing, per-element multi-transition maps)
  remains open as an H2 follow-on; the current clock keeps its
  one-transition-per-longhand shape.
- **H3 - rule object model: landed 2026-07-22.** Livery's
  `Stylesheet` now retains a CSSOM-shaped object model (the fork's
  `stylesheets/` CssRule shape sized to the lane): ordered top-level
  `CssRule::Style/Media/Keyframes` items, with `MediaRule` holding its
  nested rules and the flattened cascade/keyframes views derived caches.
  Mutation is `insert_rule`/`delete_rule` with CSSOM index and error
  semantics (IndexSize, whole-rule Syntax rejection), a monotonic
  `generation()` stamp as the StylesheetSet dirty-tracking shape, and
  reindexing that renumbers flattened source order exactly as a fresh
  parse would. genet-livery's `StyleSet` retains the UA and author sheets
  and rebuilds its cascade views on mutation; `LiveryDocument` exposes
  `insert_author_rule`/`delete_author_rule` (restyle on next frame) and
  `computed_style(node, property)`, the getComputedStyle backing:
  longhands serialize through the generated tagged reads, `--names`
  answer from the element's custom-property map, and styles resolve on
  demand when no layout is retained. Receipts: livery stylesheet wall 8
  (object-model identity, insert reindexing, media-group delete, error
  paths without generation bumps) and the new genet-livery cssom wall 5
  (computed serialization incl. custom properties, insert/delete
  restyling the retained document, inserted media rules respecting the
  device, mutation errors leaving the document intact); full walls 158
  green, livery clippy `-D warnings` clean. The scripted leg is now
  registered through an engine-neutral `StyleSheetHandler` in
  script-runtime-api and genet-scripted's opt-in `LiveryCssom` session.
  It retains Livery's author sheets beside the runtime's live DOM and
  backs `document.styleSheets`, `CSSStyleSheet.insertRule/deleteRule`,
  and `getComputedStyle` (longhands and custom properties) in Boa; the
  same CSSOM host contract passes on Nova. The WPT runner's existing
  `--renderer livery` switch now selects this style route for
  testharness runs as well as reftests. Its retained Stylo session still
  supplies geometry, hit testing, and animation cadence until Livery
  replaces that half. Receipts: 181 tests across script-runtime-api and
  genet-scripted green; a WPT-harness composition test performs a
  var()-driven insert/read/delete cycle through Boa; and the upstream
  `css/css-variables` directory is now a runnable baseline: 8 all-pass,
  46 with failures, 1 errored, 2 no-results, 185 skipped across 242
  discovered files, 194/506 subtests passing. Remaining CSSOM breadth is
  live rule wrapper objects, script-created/linked stylesheet discovery,
  and finer CSSStyleDeclaration parsing/serialization. Those limits do
  not reopen the retained rule/session bridge.
- **H4 - invalidation: landed 2026-07-22.** The fork's stable shapes were
  harvested without copying its 6.8k-line Stylist/SelectorMap-coupled tree.
  genet-livery's engine-neutral `IncrementalStyle` retains the computed
  plane, coalesced `ElementSnapshot`s (including the first old attribute
  value), restyle roots, and a `RestyleStats` receipt. Livery selector lists
  carry conservative sibling/structural dependency summaries: ordinary
  attribute and state changes recascade the changed subtree; sibling
  selectors widen to its parent; child-list mutations widen only when
  structural selectors require it. Stylesheet generation, device changes,
  explicit invalidation, and a missed scripted mutation range take the
  full-document correctness path and report that fact. Scoped selector
  identity storage includes only roots, descendants, required ancestors,
  and sibling neighborhoods. `LiveryDocument` now retains this session
  between frames. genet-scripted observes the mutable DOM's absolute
  mutation sequence without consuming Stylo's layout batch, so synchronous
  `getComputedStyle` reads stay scoped; if layout drains unseen facts first,
  the sequence gap is detected and recovered by a loud full restyle.
  Receipts: four invalidation tests diff every attached element against a
  fresh full cascade for class/attribute and state edits, exercise
  `:last-child` through insertion and removal, assert scoped counts, and
  make stylesheet-wide work explicit; selector dependency classification
  is separately guarded; scripted receipts cover both the scoped branch and
  the missed-range recovery path. Full livery + genet-livery wall: 163
  green. Full script-runtime-api + genet-scripted wall: 183 green; mutable
  DOM wall: 29 green; WPT harness wall: 23 passed, 3 intentional ignores.
  Touched native and scripted crates are clippy-clean under `-D warnings`
  after narrowly allowing named pre-existing debt. The invalidation win is
  style recascade only: retained
  `LiveryDocument` still relays out and repaints a complete frame after a
  style change, and scripted WPT geometry remains on the retained Stylo
  session.
- **H5 - value families on demand: active.** The first transform slice landed
  2026-07-22. Livery now parses `matrix()`, `skew()`, `skewX()`, and `skewY()`
  beside its existing translate/scale/rotate list, composes the bounded list
  through a public 2D affine matrix, and serializes the resolved transform as
  `matrix(a, b, c, d, e, f)` for CSSOM. The matrix module keeps its MPL header
  and names the fork's `values/animated/transform.rs` at `b157d92526`; its 2D
  decomposition/recomposition path handles mismatched transform lists and
  `none` normalization in retained transitions. Computed translation lengths
  resolve after font size, and the same matrix crosses into neutral paint.
  Receipt: the upstream
  `css/css-transforms/transform-2d-getComputedStyle-001.html` file moved from
  0/5 to 5/5 on Boa with `--renderer livery`; native unit/integration receipts
  cover affine round trips, skew and raw-matrix paint, resolved `em`
  translation, and a live translate-to-scale transition. The full livery +
  genet-livery wall is 169 green; the scripted/runtime wall remains 183 green,
  mutable DOM 29 green, and the WPT harness 23 passed/3 intentional ignores;
  touched native crates are clippy-clean under `-D warnings` with the named
  pre-existing too-many-arguments allowance. This is deliberately the 2D
  Level 1 matrix slice; its percentage follow-on is recorded below. 3D and
  perspective, individual transform properties, and the Web Animations/
  CSS-global JS surface remain open. Full `calc()`, grid template grammar, and
  font machinery are the other H5 families. This first harvested source also
  triggers the planned package-level
  license flip: `livery` now declares MPL-2.0 while retaining file provenance.
  Receipt per completed family remains a directory-level WPT delta.
  The second transform slice landed the same day: translate arguments now use
  the shared length-percentage value, with font-relative terms resolved at
  computed-value time and percentages held for the consuming reference box.
  Neutral paint resolves them against the actual fragment border box; CSSOM can
  produce the same matrix for a definite unadorned box without pretending it
  retained layout, and otherwise preserves the authored form. Percentage-to-
  percentage retained transitions stay percentage-valued until paint. The
  focal upstream `transform-percent-003.html` reftest moved from a localized 2%
  pixel mismatch to pass, and all eight applicable static reftests in the
  numbered `transform-percent-001` through `-010` series pass (009 is SVG
  testharness; 010 requires scripted reftest mutation). Native receipts cover
  value resolution, CSSOM, retained interpolation, and paint; the full livery +
  genet-livery wall is now 173 green, the three scripted Livery bridge tests and
  WPT CSSOM composition test remain green, and native clippy stays clean under
  the existing named allowance. Remaining transform work is mixed length/
  percentage interpolation through calc, calc expressions inside transform
  functions, transform-origin/transform-box and SVG reference boxes, 3D and
  perspective, and individual transform properties. Transform calc stays open
  because the bounded transform-list parser is not nested-function-aware yet.
  The third H5 slice landed the same day: `values/calc.rs` harvests Stylo's
  precedence and dimensional-arithmetic shape from
  `style/values/specified/calc.rs` at `b157d92526`, with its MPL header and
  provenance note, then reduces the current length-percentage lane to a compact
  linear form. Nested `calc()` and parentheses, sums, number products, and
  division by a nonzero number now parse with dimensional rejection and
  canonical unit ordering. The engine-neutral `InlineStyleHandler` gives
  `element.style` three explicit outcomes: canonical, invalid, or pass-through.
  Livery installs it and canonicalizes only successfully parsed implemented
  longhands plus the bounded border shorthand's math width; unknown properties,
  other shorthands, `var()` values, and grammar beyond its lane stay authored
  rather than being discarded as invalid. The contract passes on Boa and Nova.
  The follow-on closed the same upstream
  `css/css-values/calc-nesting.html` receipt from 6/8 to 8/8. The engine-neutral
  DOM bootstrap now publishes non-colliding parse-time element ids as named
  globals on both engines. Livery's preliminary layout resolves mixed
  length-percentage `calc()` sizes against a known containing block, and the
  scripted CSSOM supplies the resulting fragment size when width or height
  needs a used pixel value for an unadorned box. The border serializer reduces
  its nested calc width without rewriting the authored style or color token.
  The fourth H5 slice landed 2026-07-23: `vw`, `vh`, `vmin`, and `vmax` now
  remain independent terms through specified calc algebra and canonical
  serialization, then resolve from Livery's current `Device` at the
  specified-to-computed boundary before font metrics, layout, transitions, or
  paint consume them. Generated `PropertyValue` dispatch requires every value
  family to state whether and how it resolves viewport units; an unresolved
  viewport length reaching a downstream px-only consumer fails loudly. A
  retained device resize recascades the style plane and recomputes the units.
  The upstream `css/css-values/calc-serialization.html` receipt moved from 0/1
  to 1/1 while `calc-nesting.html` remains 8/8. Native tests cover all four
  unit bases, canonical term order, computed font, transform, and grid-track
  values, layout geometry, and device changes. Native Livery's full wall is 179
  green; the full script-runtime-api + genet-scripted wall is 188 green. Both touched
  walls are clippy-clean under `-D warnings` with the named pre-existing
  allowances. The fifth H5 slice landed 2026-07-23. Livery now parses and
  canonically retains all 24 physical and logical viewport units: the default,
  small, large, and dynamic `vw`/`vh`/`vi`/`vb`/`vmin`/`vmax` families.
  `Device` carries distinct small, large, and dynamic metrics, and scripted
  device changes invalidate the computed result without collapsing those
  tiers. The six `cq*` units remain deferred through cascade. A preliminary
  layout then selects the nearest `container-type: inline-size` or `size`
  ancestor independently for each axis, resolves against its content box, and
  uses the small viewport for any axis without an eligible container. The
  final style plane retained by layout and exposed through `getComputedStyle`
  contains the resolved result. Direct atomic `min()`, `max()`, and `clamp()`
  arguments share that deferred environment and percentage-basis path; this
  bounded representation accepts up to eight comparison arguments and eight
  distinct environment-relative terms in one linear `calc()`. Native and Boa
  receipts cover distinct viewport tiers, logical axes under the current
  horizontal writing mode, independent nested container axes, content-box
  sizing, fallback, comparison ordering, comments, percentages, mutation, and
  retained/on-demand computed style. Native Livery's full wall is 183 green;
  the full script-runtime-api + genet-scripted wall is 189 green. Both touched
  walls are clippy-clean under `-D warnings` with the named pre-existing
  allowances. The upstream `calc-serialization.html` and `calc-nesting.html`
  receipts remain 1/1 and 8/8. The broader
  `viewport-units-compute.html` fixture is still blocked before value reads by
  its iframe `contentWindow`/`contentDocument` dependency, and
  `clamp-length-computed.html` is blocked by computed
  `CSSStyleDeclaration` property-membership support before it reaches the
  clamp assertions.

  The sixth H5 slice landed 2026-07-23. Generated inherited `writing-mode`
  values now select physical and logical viewport/container axes for each
  element. The bounded math representation is a compact postfix evaluator
  rather than one atomic comparison node: nested `calc()`/`min()`/`max()`/
  `clamp()`, arithmetic operands, length division, and `none` clamp bounds
  resolve through the same delayed environment. `container-name` and retained
  `@container` objects cover named width/height/inline-size/block-size
  comparisons, colon and chained ranges, and `and`/`or`/`not`; layout recascades
  size queries to stability with an eight-pass cycle bound. The vendored Taffy
  seam excludes contained child content from intrinsic physical axes for
  block, flex, and grid, with vertical inline-size containment mapped to
  height. Script now supplies computed-style membership, `CSS.supports`,
  `innerHTML`, and initial same-origin iframe documents whose child Livery
  cascade uses the embedding frame's content-box viewport. CSSOM insertion and
  deletion of named container rules is live.

  Receipts: native Livery is 187 green; script-runtime-api + genet-scripted is
  193 green. The upstream Boa/Livery files pass
  `calc-serialization.html` 1/1, `calc-nesting.html` 8/8,
  `clamp-length-computed.html` 24/24, and
  `viewport-units-compute.html` 34/34. Native, scripted, Taffy, and
  stylo_taffy touched surfaces are clippy-clean under `-D warnings` with only
  the named pre-existing allowances.

  The remaining value boundary is stepped, trigonometric, and exponential
  math; style and scroll-state container queries; nested media/container
  grouping and fuller query grammar; cycle diagnostics and
  `contain-intrinsic-size`; general shorthand reconstruction; nested transform
  arguments; and used-value serialization for adorned boxes and more layout
  properties. Iframe navigation, origin policy, and independent event loops
  remain browsing-context work beyond the initial child-document surface.
- **H6 - media tiers come home.** Re-express the Mark-authored fork media
  tiers on Livery's Device under MIT/Apache. Receipt: media-query WPT
  parity between the Livery and fork routes.

Order: H0 first. H1 and H2 are independent. H3 precedes H4 (invalidation
consumes the rule model). H5 is continuous. H6 is anytime. The pivot tier
for the climb is backing genet-scripted (CSSOM, getComputedStyle,
invalidation); H3+H4 are its core.

## The fork's demotion, and its retirement trigger

The fork stays, as incumbent and quarry only. genet-layout consumes
genet-stylo 0.19.1 on the fullweb lane today; the carried tiers are real
capability upstream lacks; the rename family keeps published genet crates
resolvable. Carrying cost after Track U is about nine commits realigned
per tagged release. What ends is the fork as a project: no crate split,
no product-lane pruning, no new divergence beyond keeping the incumbent
green. **Retirement trigger:** Livery takes the fullweb default with WPT
parity receipts; then the fork repo archives and the genet-stylo publish
family freezes at its last release.

## Non-goals, named

- mako and `build.py` are quarry documentation, never Livery build
  machinery; Livery's Rust generator over TOML is the identity.
- No gecko/ glue, no rayon parallel traversal, no to_shmem.
- No rule tree. The MatchedPropertiesCache shape remains the planned
  sharing story when a measured consumer needs one.
- cssparser and selectors stay shared dependencies, lifted nowhere.
