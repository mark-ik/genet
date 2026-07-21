# Stylo harvest into Livery: the climb to fullweb

**Date:** 2026-07-20
**Status:** H0, H1, and H2 landed 2026-07-20; H3+ not started. Census
grounded against the local fork checkout this session.
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
- **H3 - rule object model.** Lift the `stylesheets/` CssRule model and
  StylesheetSet dirty tracking as CSSOM backing. Receipt:
  insertRule/deleteRule and getComputedStyle serialization exercised
  through genet-scripted against the Livery style plane on a bounded
  corpus.
- **H4 - invalidation.** Lift `invalidation/element/` with element
  snapshots and restyle hints, plus stylesheet invalidation. This is the
  scripted-tier keystone and the hardest graft (coupled to Stylist and
  SelectorMap shapes). Receipt: class/attribute/state mutations restyle a
  scoped set, with a diagnostic asserting restyle counts so O(document)
  restyles are loud, not silent.
- **H5 - value families on demand.** Transforms (matrix decomposition),
  full calc(), grid template grammar, font machinery, as the WPT ratchet
  reaches each. Receipt per family: directory-level WPT deltas.
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
