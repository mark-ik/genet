# The stylo fork: decomposition and opportunistic divergence

**Date:** 2026-07-13
**Status:** research + proposed tracks (Mark's framing: "we have a fork, and
we should use it," opportunistically taking from upstream as with every
fork). Grounding facts verified against the fork and serval this session.

Coordinates with the in-flight ring-3 fork rename
([2026-07-12_ring3_fork_rename_publish_plan.md](./2026-07-12_ring3_fork_rename_publish_plan.md)):
the local checkout sits on `mark-ik/serval-publish-names` (stylo →
`serval-stylo` and kin). Everything here that moves files LANDS AFTER that
rename, on the renamed family, so the two surgeries never collide.

## The fork today (verified 2026-07-13)

- **Divergence is shallow**: ~9 real fork commits on
  `mark-ik/servo-media-features` atop servo's stylo — the media-feature
  tiers A–E (the fork's identity: geometry, device-capability,
  accessibility, display-mode/scripting, prefers-reduced-motion,
  multi-capability pointer/hover) plus animation edge-fixes. Everything
  else in `upstream/main..` is servo's own downstream. Merges are cheap
  today; every proposal below is priced against keeping them cheap-ish.
- **Anatomy**: `style` is the 75.6k-LOC monolith; inside it `values/` is
  47.8k (63%), `properties/` is 15.6k of source *plus mako templates that
  Python-expand at build* into the real longhand tables, `stylesheets/`
  8.4k, `invalidation/` 7k. `selectors` (9k), `servo_arc`, `style_traits`,
  the derive crates, and `to_shmem` are already separate members.
- **Gecko is cfg'd out, not gone**: `gecko/`, `gecko_bindings/`,
  `gecko_string_cache/` (~7k) compile only under the `gecko` feature, which
  serval never enables. Deleting them is hygiene, not compile speed — and
  pure merge tax. Leave them.
- **The build needs a Python interpreter (only)**: `style/build.rs` finds
  `python.exe`/`python3` (or `PYTHON3`) and runs `properties/build.py`,
  which loads a **TOML property database** (`longhands.toml`,
  `shorthands.toml`, plus descriptor TOMLs) through `data.py` and renders
  `properties.mako.rs` (~5.9k lines, with `helpers.mako.rs`) into a single
  generated file. mako + toml + markupsafe are **vendored as wheels
  in-tree** (`properties/vendored_python/`) — no pip, no packages, just an
  interpreter on PATH. Output: `OUT_DIR/properties.rs` at **97,717 lines /
  4.4 MB** — every longhand's specified/computed types, parse/serialize,
  the PropertyDeclaration/LonghandId enums, cascade tables, and the
  ComputedValues style structs serval-layout's accessors live on. So the
  crate's *effective* compile surface is ~167k lines (69k handwritten +
  98k generated), not the 75.6k the source count suggests — and a quarter
  of it is one file rustc cannot parallelize over.
- **serval-layout is the SOLE stylo consumer** in the whole engine
  (serval-render/scripted/extract: zero style imports). Its measured
  surface: 10 style structs (`get_box` 22, `get_inherited_text` 14,
  `get_background` 9, `get_position` 8, `get_effects` 5, `get_border` 4,
  then text/svg/inherited_box/font), ~33 distinct longhand accessors —
  against stylo's ~450 longhands. One crate to audit; one place to verify
  any divergence.

## Track 0 — zero-divergence wins (do first, no fork edits)

1. **Dev-profile package overrides.** Neither serval nor mere carries a
   `[profile.dev.package]` entry for the stylo family. Add
   `opt-level = 1`-style / reduced-debuginfo overrides for
   `serval-stylo` + `selectors` in serval's and mere's workspace profiles
   (the same mechanism both already use for jemalloc/num-bigint). Faster
   dev links and smaller incremental artifacts for every consumer, zero
   divergence, one commit per workspace.
2. **Timing baseline (recorded 2026-07-13).** Cold `cargo check` of the
   style crate **including its full dependency tree**, empty target, dev
   profile, on the primary Windows laptop: **30m 35s**. That is the
   whole-tree number, not style alone; the per-crate attribution needs a
   `cargo build --timings` critical-path graph for serval-layout, to be
   captured before/after each track lands so wins are receipts, not vibes.

## Track 1 — decomposition (compile speed)

**1a. Pre-generate the mako output.** Commit the expanded `properties.rs`
(97.7k lines) to the fork and reduce `build.rs` to nothing; the regen tool
already exists — it is `properties/build.py` itself, run manually after
each upstream sync instead of by cargo. Wins: no Python in anyone's build
environment (Windows-vanilla friction gone — the build-env snapshot doc's
biggest asterisk), build-script rerun invalidations gone (today *any*
touch under `properties/` reruns the whole expansion), better behavior
under any future sccache. Honest sizing: raw compile time barely moves
(the generated code still compiles); this is a build-*environment* and
cache-hygiene win. Merge cost: one script run per sync, mechanical — and
the committed output makes template-diff review *easier* (2a's product
lane shows up as a reviewable generated-code diff).

**Rejected alternative, named:** porting the codegen off mako entirely
(Rust build.rs/xtask reading the TOML database, or a proc-macro). More
tractable than it used to be — upstream moved the property database to
declarative TOML — but `properties.mako.rs` still carries real Python
logic (loops, conditionals, `data.py` class methods), so it's a genuine
port of ~6k template lines at exactly the layer upstream churns hardest.
All cost, no win over 1a: once the output is committed, mako is a
maintainer-side tool a build never sees, and staying on upstream's
generator keeps template merges trivial.

**1b. Split the monolith on its natural fault line.** Three crates:

    serval-stylo-values   (values/, color/, logical_geometry, str/parser helpers — ~48k)
      -> serval-stylo-props (the mako-generated longhand tables + cascade glue)
        -> serval-stylo      (stylist, matching, traversal, invalidation, sharing,
                              rule tree, stylesheets, media queries — the engine)

Why it pays, honestly stated: rustc pipelines *between* crates, and the
style crate is today the serialization point on serval's cold-build
critical path — one giant node nothing downstream can start behind.
Splitting lets values/props/engine pipeline, and metadata reuse means
iterating on engine code (where all nine fork commits live) stops paying
values' 48k each time. What it does NOT buy: serval-layout still rebuilds
whenever any piece changes.

**The merge tax, named:** file moves conflict with upstream merges
forever. Mitigation: keep every file's *path inside* the new crates
identical to its path in `style/` (crate roots re-export; only Cargo.tomls
and `crate::` → cross-crate paths change), so git rename detection does
most of the work and conflicts stay at the import-line level. This is the
priciest item in the plan; it should land only after 1a and only when the
fork expects sustained iteration (the property-pruning track below is
exactly that).

## Track 2 — divergences with outsized benefit (serval ≠ servo ≠ firefox)

**2a. The property lane: prune to what serval renders.** The single
biggest lever. Serval consumes ~33 longhands through 10 structs; stylo
compiles ~450 longhands, each with specified/computed/animated types,
parse/serialize impls, and derive expansions, most feeding capabilities
serval deliberately knocked out (the W3C-knockout doctrine applies: delete
now, rebuild deliberately later). Mako already supports per-product
gating — add a `serval` product lane to the templates that drops longhand
families serval-layout cannot consume (ruby, MathML-adjacent, paged/print
media, view-transitions, scroll-driven animation timelines, the long tail
of -webkit compat). Unknown properties already parse as ignored, so
pruned properties degrade to exactly what an unsupported property does in
any engine. Expected: a large fraction of the generated code and its
values types gone from the build, plus real runtime wins (smaller
ComputedValues, smaller cascade tables). Cost: template-level divergence
where upstream churns; the audit list must be data-driven (start from the
33, walk what getComputedStyle/serval-extract/scripted CSSOM expose, gate
conservatively). This is the track that justifies 1b's split.

**2b. Suppress the parallel machinery serval never uses.** Stylo carries
rayon parallel traversal + a global style thread pool; serval drives the
cascade on its own thread, single-lane. Verify the pool never spawns under
serval's usage (`STYLE_THREAD_POOL` is lazy — confirm nothing tickles it),
and if it does, gate it. Startup + memory win, near-zero merge cost.

**2c. Style-sharing cache and bloom filter, tuned for serval's DOMs.**
Chrome DOMs are tiny; smolweb documents are simple; meerkat cards are
shallow. The sharing cache and bloom setup are tuned for Gecko-scale
documents and may be pure overhead below some node count. Measure-first
divergence: adaptive skip under a threshold. Runtime win only.

**2d. `to_shmem` under the servo feature.** The shared-memory UA-sheet
machinery is Gecko's; every property struct still derives it. Investigate
whether the servo feature can drop the derives (proc-macro expansion +
compile time across the biggest structs in the tree). Might be upstreamable
by Servo someday, but per doctrine we just take it in the fork if it's real.

**2e. Not touching:** `selectors` (upstream-hot, shared, correctness-dense),
the rule tree and cascade core (same), custom properties/@property
(recently fixed upstream and the web needs them).

## Upstream-sync posture

Opportunistic, as with every fork: merge upstream *before* each big track
lands (cheapest point), regenerate mako output after every sync (1a's
script), and keep fork commits in the engine half where the 1b split
concentrates them. The no-upstreaming doctrine stands — these divergences
are ours.

## Order and done conditions

1. Track 0.1 profile overrides land in serval + mere (receipt: link-time
   delta on `cargo build -p serval-layout` dev).
2. Track 2b verification (receipt: no style pool thread in a serval run's
   thread list).
3. Track 1a mako pre-generation (receipt: fresh clone builds with no
   Python on PATH).
4. Property audit for 2a: the definitive consumed-longhand list, from
   serval-layout + CSSOM exposure (receipt: a checked-in audit table).
5. Track 1b split, after the ring-3 rename lands (receipt: cold-build
   `--timings` before/after; the style node leaves the critical path).
6. Track 2a serval product lane, gated by the audit (receipt: generated-
   code line count + cold build + ComputedValues size, before/after).
7. 2c/2d as measure-first follow-ons.

## Open items

- Ring-3 state as of 2026-07-13: the rename branch
  (`mark-ik/serval-publish-names`, efaa436663) is pushed; T0–T3's
  engineering landed and verified per the ring-3 plan; **nothing
  published** — `serval-stylo`/`serval-taffy` don't exist on crates.io,
  not even as name claims. Publishing stays Mark's per-crate call.
- Whether stylo_taffy's vendored copy (support/patches/stylo_taffy) rides
  the renamed family cleanly after ring 3 — it already git-deps
  serval-stylo on the rename branch; the decomposition must keep it
  building.
