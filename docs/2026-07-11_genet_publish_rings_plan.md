# Publishing genet: rings, outside-in, consumer-pulled

**Date:** 2026-07-11
**Status:** plan, proposed. Dependency facts verified against source this
session (stylo exposure, git-dep exposure, package names, netrender/
netfetcher purity).

Companion to
[2026-07-09_inker_genet_adoption_plan.md](./2026-07-09_inker_genet_adoption_plan.md)
(whose republish landed the adopted family + three name-claim stubs this
plan retires ring by ring).

## The question

The adopted family (inker, nematic, errand, verso-tile, illume, tinct, the
protocols) is published. Should the genet engine itself follow — and how?

Genet is not the adopted family: it is a Servo fork whose core builds on
**forked upstream dependencies** (the mark-ik/stylo branch, a taffy patch,
boa and nova fork branches, gpu-allocator/sonic-rs patches) applied through
`[patch.crates-io]` and git pins. Published crates carry neither patches nor
git deps, so any component whose build needs a fork **cannot publish
faithfully** — a registry consumer would silently build against upstream.
That line, not licensing (MPL-2.0 is fine) and not effort, divides genet
into rings.

## Verified facts

- **Stylo-free and git-dep-free** (checked per manifest):
  engine-observables-api, layout-dom-api, paint-types, script-engine-api,
  genet-static-dom, genet-scripted-dom, genet-static-html,
  genet-extract, pelt-core.
- **xilem-serval carries zero stylo/genet-layout deps.** Its one wrinkle is
  the vendored `xilem_core` (a verbatim Apache mirror of upstream 0.4.0):
  the name is Linebender's on crates.io, so publishing means either depping
  the *published* upstream xilem_core (preferred if still verbatim) or
  renaming the vendor.
- **netrender and netfetcher have zero git deps** in their own repos:
  publishable as-is under Mark's names.
- **Name collisions exist only in the deep cone**: `servo-*` package names
  (servo-paint-types, servo-embedder-traits, ...) sit in Servo's crates.io
  namespace and would need `genet-*` renames at publish time; the vendored
  `xilem_core` likewise.
- The stylo family pins (`stylo`, `stylo_atoms`, `stylo_dom`,
  `stylo_malloc_size_of`, `selectors`, `servo_arc`) all point at the
  mark-ik/stylo fork branch; `taffy` is a local patch. These names are
  upstream's — a fork can never publish under them.

## The rings

**Ring 0 — contracts + DOM column (publishable now).**
engine-observables-api, layout-dom-api (real 0.1.0, superseding the 0.0.1
stub), genet-scripted-dom (real, superseding its stub), genet-static-dom,
genet-static-html, genet-extract, script-engine-api, pelt-core, and
paint-types renamed `genet-paint-types` if the servo- name collides.
Pull that justifies it today: verso-tile's `genet-donor` feature becomes
real for registry consumers the moment its two deps are real, and the stub
debt from the adoption republish starts retiring.

**Ring 1 — the sibling engines, from their own repos.**
The netrender family (netrender, netrender_device, netrender_text,
paint_list_api, paint_list_render) and netfetcher. Zero git deps, own
names, no fork entanglement. Unlocks document-canvas's real publish (its
netrender git deps become version deps) and makes genet-winit-host a
candidate. Pull: strophe/isometry/woodshed consuming the paint engine
without a genet checkout.

**Ring 2 — the host-framework column.**
xilem-serval (real, superseding the stub; resolve xilem_core per above),
then chisel, document-canvas, knot-editor-host. Pull: `nematic::views`
becomes real for registry consumers — the largest single upgrade the stub
pattern is holding open.

**Ring 3 — the wall (stays git-native, deliberately).**
Everything downstream of the forks: genet-layout and genet-render (stylo
+ taffy), genet-scripted and the boa/nova engine wrappers (fork branches),
genet-documents (deps the layout cone), pelt-desktop/pelt, genet-wpt.
These publish only if the fork family itself publishes under its own names
(genet-stylo and kin — mechanical but heavy, and never by upstreaming;
the no-upstream-PRs doctrine stands). Trigger to revisit: an external
consumer actually asking for the layout cone from the registry. Until
then, git deps are the honest interface, and the family's tooling (local
patches, branch tracking) is built for it.

## Policy points

- **Consumer-pulled, not completionist.** A ring publishes when something
  real pulls it, in ring order. Ring 0's pull exists today.
- **Versioning:** published crates leave the workspace's lockstep `=0.2.0`
  style and take independent semver from 0.1.0, path+version workspace
  entries exactly like the adopted family.
- **Stubs retire ring by ring**: each real publish supersedes its 0.0.1
  claim (layout-dom-api and genet-scripted-dom in ring 0, xilem-serval in
  ring 2). No new stubs unless a new dependent's feature demands one.
- **Renames happen at publish, not before**: servo-* package names become
  genet-* only when their ring goes; in-tree consumers keep the rename
  invisible via workspace `package =` aliases.

## Progress

- **2026-07-11, rings 0-2 PUBLISHED** (17 crates this pass; genet commits
  d17bc307060 + follow-ups, netrender 6fd8cd4c5 pushed, netfetcher pushed):
  - Ring 1: paint_list_api, netrender_device, netrender, netrender_text,
    paint_list_render, netfetcher — all 0.1.0.
  - Ring 0: layout-dom-api 0.1.0 and genet-scripted-dom 0.1.0 (stubs
    superseded), genet-paint-types 0.1.0 (renamed from servo-paint-types),
    engine-observables-api, genet-static-dom, genet-extract,
    script-engine-api, pelt-core — 0.1.0.
  - Ring 2: serval-xilem-core 0.4.0 (the vendored mirror published under
    the genet prefix after the registry-0.4.0 swap failed — the vendor
    tracks upstream main past the release), serval-chisel 0.1.0 (chisel is
    taken on crates.io; lib names unchanged for both renames), and
    xilem-serval 0.1.0 (stub superseded). All three name-claim stubs from
    the adoption republish are now retired by real crates, so nematic's
    `views` and verso-tile's `genet-donor` resolve real from the registry
    with no dependent republish (the >=0.0.1 reqs pick the new versions).
  - **genet-static-html reclassified to ring 3** mid-pass: its
    servo-layout-api dep is the semantic fork (a registry build silently
    resolves Servo's published crate and loses LayoutHostServices) — the
    exact hazard this plan's fork line names, caught live by the publish
    verify. Audit lesson recorded: manifest greps miss workspace-alias
    deps; `cargo tree -i <fork-crate>` is the audit tool.
  - **Known heaviness, follow-on**: the published contracts reach upstream
    stylo through servo-malloc-size-of (a fork-invariant trait surface —
    compiles correctly, but drags the upstream stylo build into registry
    consumers). A 0.1.1 pass feature-gating malloc_size_of across
    engine-observables-api / genet-paint-types / genet-scripted-dom would
    slim it; 92 references, so it is a deliberate refactor, not a tweak.
  - The netrender-family git deps at genet's root now carry version reqs
    (git + version: local builds keep git resolution, publishes record the
    registry req) — this required pushing netrender/netfetcher, and note
    that cargo's `paths` override does NOT apply to git deps.

## Done conditions

1. Ring 0 published; verso-tile 0.1.1 republished with real (not stub)
   donor deps resolving; the two ring-0 stubs marked superseded in their
   registry descriptions.
2. Ring 1 published from netrender/netfetcher repos; document-canvas
   republished with version deps.
3. Ring 2 published; nematic 0.1.x republished with a real xilem-serval
   req; the views feature verified building from a registry-only consumer.
4. Ring 3: no action; the trigger and the renamed-fork answer are recorded
   here so the wall is a decision, not a surprise.
