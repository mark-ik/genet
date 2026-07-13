# A second CSS engine: prior art and growth plan

**Date:** 2026-07-13
**Status:** research + proposed staging. Mark's framing: "grow a rust
alternative using firefox, chrome, servo, blitz, ladybird, and gosub as
prior art... think it'd be neat to have two css engines. seems like
that's what we do around here."
**Companions:**
[2026-07-13_stylo_fork_decomposition_and_divergence_plan.md](./2026-07-13_stylo_fork_decomposition_and_divergence_plan.md)
(the fork stays the full-fat engine; the two plans share their first
deliverable, the consumed-property audit),
[2026-07-02_gosub_lessons.md](./2026-07-02_gosub_lessons.md) (the
existing gosub harvest).

## Why two engines

It is the established posture, not an exception: boa + nova on the JS
lane, scrying/grafting/weld behind `SurfaceEngine`, netrender's
pluggable backends, three engine kinds in inker. The pairing here:

- **serval-stylo (the fork)** stays the web-correct engine for the
  fullweb lane — 450 longhands, spec-hardened, opportunistically merged
  from upstream.
- **The lean engine** is grown, not ported: a serval-native cascade
  sized to what serval actually renders (~33 longhands through 10 style
  structs today), owned end to end, no gecko residue, no Python, no
  75.6k-LOC monolith on the critical path.

The candidate first consumers make the split concrete: host chrome
(xilem_serval UI), smolweb documents, and card-sized content are small
DOMs with small property needs that currently pay for the whole stylo
build and runtime. Fullweb pages keep stylo. Per-document engine choice
is the same shape as the browser multiplexer.

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
model here — and serval already owns the ratchet (the WPT baselines +
reftest harness serval-layout runs today).

**Gosub** — the warning label, already harvested in the gosub lessons
doc: its parsers "exist to be independent, not better." Independence is
not a reason. The lean engine is justified by *fit* (lean property set,
serval-shaped Device/media integration, owned growth curve), or not at
all. Gosub's transferable piece (gosub_lattice table layout) belongs to
serval-layout regardless of engine.

**Blitz** — not an alternative engine; the other Rust *consumer* of
stylo (with taffy/parley/vello, the same decomposition mere uses). Its
role here is the null hypothesis: stylo-as-dependency is viable and is
what serval does today. The lean engine must beat it on the measures
that matter to us — cold build (30m35s whole-tree baseline), dev
iteration, binary size, and API fit — or it doesn't deserve to exist.

## The head start: the substrate is already shared crates

A CSS engine is five things, and two of them are free:

1. **Tokenizer/parser** — `cssparser` is a standalone crate (stylo
   itself consumes it). Shared.
2. **Selector matching** — `selectors` is a standalone crate, already a
   fork-family member serval builds. Shared, including specificity and
   the invalidation-relevant hashes.
3. **Property system** — the property DB + codegen + specified/computed
   value types. **This is the engine.** Built new, in Rust: a TOML
   property database (upstream stylo itself moved to TOML — same shape)
   and a Rust generator (build.rs or committed-output xtask, no
   Python). This is where "port the codegen off mako" lands: not as a
   stylo retrofit (rejected in the fork plan) but as the lean engine's
   foundation, sized to serval's audit instead of Gecko's 450.
4. **The cascade** — origin/layer ordering, specificity application,
   inheritance, initial values, importance. Small once the property
   system exists; spec-literal first, Ladybird-style.
5. **Media evaluation** — serval already owns opinions here (the fork's
   media-feature work, the `MediaEnvironment` consolidation); the lean
   engine gets a Device shaped for serval hosts from day one.

Value types are the honest hard part: even 33 longhands need lengths,
percentages, calc(), colors, and round-trip serialization. Stylo's
values/ is 47.8k LOC for the full set; the lean subset is thousands of
lines, not hundreds. The win is not avoiding CSS — it is avoiding the
~90% serval doesn't consume, plus owning the growth curve.

## The seam: how serval-layout hosts two engines

serval-layout today imports `style::` directly (67 value imports, 10
style structs, 33 longhand accessors). Computed styles are hot-path
data, not behavior — a trait-object seam per style read is the wrong
tool. The Rust-y seam is type-level: the lean engine exposes a
`ComputedValues` with the **same 33 accessor names and value types**
serval-layout already calls (`get_box().clone_display()`, ...), and
serval-layout selects its engine by cargo feature. Costs stated: the
accessor-compatible surface couples the lean engine's API to stylo's
shape. Accepted deliberately — it is 33 accessors, it makes the engines
swappable without rewriting serval-layout, and a neutral
`layout-style-api` contract crate (the layout-dom-api precedent) can
graduate out of it later if the surface stabilizes.

The consumed-property audit (Track 2a of the fork plan) is therefore
the shared first deliverable of BOTH plans: it is the lean engine's
property spec and the fork's pruning list. Do it once. And the two
tracks trade off: if the lean engine takes the chrome/smolweb/card
lanes, stylo pruning (fork Track 2a) matters less — stylo can stay
fat for fullweb only. Choose per-lane after the audit, don't do both
blindly.

## Licensing and home

**Clean-room, MIT/Apache-2.0, edition 2024** (founding convention). No
stylo/Gecko code may be ported in — MPL infects files it derives from.
Reading Ladybird (BSD-2) and gosub (MIT) is fine; *depending on*
cssparser/selectors (MPL-2.0) is fine (file-level copyleft does not
reach dependents); copying stylo source is not. This is also the
practical reason the engine is a new codebase rather than a stripped
fork: a stripped stylo is forever MPL and forever merge-taxed;
the lean engine is neither.

**Home:** start as a serval component (`components/`, publish = the
usual rings posture) for the tight edit loop with serval-layout, with
the extraction-to-sibling-repo option held open once it stabilizes —
both patterns are established. Naming: TBD with Mark (product-name
lane); this doc deliberately says "the lean engine."

## Stages, each with a receipt

- **E0 — audit + database.** The consumed-property audit (shared with
  the fork plan) becomes `properties.toml`: name, inherited?, initial
  value, value type, animation class. Receipt: the checked-in table,
  reviewed.
- **E1 — codegen + values.** Rust generator emits property enums,
  ComputedValues structs (33-accessor-compatible), initial/inheritance
  tables; the lean values layer (lengths, percentages, calc, color,
  keywords) with serialization round-trip tests. Receipt: generated
  code compiles standalone; round-trip property tests green.
- **E2 — cascade + media.** Matching via `selectors`, cascade
  (origins, specificity, importance, inheritance), media evaluation on
  a serval-shaped Device. Receipt: a hand-built corpus of
  style-resolution unit tests (Ladybird-style spec-literal cases).
- **E3 — serval-layout behind a feature.** The engine feature lands;
  the chrome/smolweb reftest corpus passes identically under both
  engines. Receipt: reftest parity run, plus the build-time delta
  (whole-tree cold check with stylo out of the graph vs the 30m35s
  baseline).
- **E4 — first production lane.** One real lane (host chrome or
  smolweb) ships on the lean engine by default. Receipt: the lane's
  existing suites green + capture receipts.
- **E5+ — grow by ratchet.** New longhands enter via properties.toml +
  WPT/reftest additions, Ladybird-style. Fullweb stays stylo until the
  audit says otherwise.

## Non-goals, named

- Not a stylo replacement on the fullweb lane — the fork plan stands,
  including realignment onto upstream releases.
- No parallel traversal, no style sharing cache, no rule tree in the
  first cut — small-DOM-first; add sharing/memoization
  (MatchedPropertiesCache shape first) only when a measured consumer
  needs it.
- No independent selector or parser implementations (gosub's mistake) —
  cssparser/selectors are shared substrate.
- No custom properties in E0–E3; staged in when a consumer lane needs
  them (chisel theming is the likely trigger).
