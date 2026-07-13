# Inker adoption: the engine-management layer moves into genet

**Date:** 2026-07-09
**Status:** plan, direction endorsed (session discussion 2026-07-09). Dependency
findings verified against source the same day; no code has moved yet.

Companion to [2026-07-09_native_automation_plan.md](./2026-07-09_native_automation_plan.md)
(pelt is the reference shell both plans converge on) and mere's
2026-07-09 mere/merecat boundary pass plan (this move removes inker from that
boundary question entirely).

## Motivation

Inker routes every content lane the family renders (`genet.*`, `nematic.*`,
`scrying.web`, `graft.servo`, `weld.chromium`), but it can only *manage* what
implements its traits, and the genet lanes do not: `genet.web` and
`genet.scripted` are routing constants with no `impl Engine` behind them, so
meerkat hand-dispatches those lanes in its content actor. Writing those impls
is impossible from either side today (a genet-side impl would dep mere; a
mere-side impl re-creates meerkat's hand-wiring for every app).

Moving inker into genet fixes the layering. The stack becomes:

    genet components (layout, render, scripted-dom, ...)
      -> inker (contracts + document/surface registries)
        -> engines (nematic in-repo; scrying/graft/weld adapters)
          -> pelt (reference shell)
            -> apps (merecat's mere, strophe, isometry, woodshed)

Pelt's three lanes (static, scripted, smolweb) get real `impl Engine`
wrappers in-repo. Apps register engines and hand frames to their compositor;
engine choice becomes a per-app build condition. This also makes the code
match the endorsed multi-engine multiplexer framing (scry/graft/weld as
pluggable `SurfaceEngine`, scry default), which was always genet-level.

License is the quiet enabler: inker and nematic are published (crates.io,
0.0.1) as MPL-2.0, and genet is the family's one MPL workspace. Moving them
here needs no relicensing; moving them to any MIT/Apache sibling would.

## What moves

| Crate | From | To (proposed) | Notes |
| --- | --- | --- | --- |
| inker | mere/crates/inker | genet/components/inker | minus the statements apply-half, minus kernel dep (below) |
| nematic | mere/crates/inker/engines/nematic | genet/components/nematic | top-level component (an engine family in its own right, not an inker internal); deps: inker, errand, pulldown-cmark, jotdown; clean |
| document-canvas | mere/crates/inker/document-canvas | genet/components/inker/document-canvas | deps: inker, parley, netrender family; genet already carries all of these |
| knot-editor-host | mere/crates/inker/knot-editor-host | genet/components/inker/knot-editor-host | deps: illume, inker, nematic, pulldown-cmark; clean |
| scrying-engine / graft-engine / weld-engine | mere/crates/inker/engines/* | genet/components/inker/engines/* | dep the standalone wgpu-* crates.io libs; clean |
| verso + verso-api + verso-scry + verso-genet | mere/crates/verso* | genet/components/verso-tile (one crate; see below) | consolidated under the already-claimed crates.io name |

Inker's engines/ subtree keeps the three surface adapters; nematic promotes
to a top-level component. Genet's workspace lists each member individually
as usual.

## The two decouplings

Inker core's only non-trivial dependency is `kernel`, used in exactly two
places.

**1. routing.rs ID types.** `routing.rs` imports `GraphViewId` and `NodeKey`
as `Option<>` context fields on a route request (per-node engine pins).
`NodeKey` is a type alias for petgraph's `NodeIndex`; `GraphViewId` is a Uuid
newtype. Following the seiche precedent (its `NodeKey` comes from petgraph
directly, kernel-free), inker takes `NodeIndex` from petgraph and defines its
own view-id newtype; mere converts at the boundary. This also drops the
dev-dep on kernel fixtures once routing tests build their pins from plain
values.

**2. statements.rs splits.** The module is already internally split:
`link_statements` (pure block walk, no kernel) and `apply_link_statements`
(asserts kernel `Semantic` edges). The pure half travels with inker. The
apply half stays mere-side; it has zero consumers today (it is staged
material for the linked-data plan), so its destination is decided when that
plan gives it a consumer, likely mere's linked-data crate.

## Routing constants cleanup

App-flavored IDs (`graphshell.internal`, `linked-data.ingest`,
`host.external-protocol`) move to app-side constants; genet's routing
vocabulary should not name mere concepts. Registry keys are plain strings, so
apps defining their own constants costs nothing. The engine-shaped IDs
(`genet.*`, `nematic.*`, `scrying.web`, `graft.servo`, `weld.chromium`) stay
with inker.

## Publish mechanics

Genet's workspace sets `publish = false`; inker and nematic carry per-crate
`publish = true` overrides, updated `repository` fields
(mark-ik/genet), and a version bump (0.0.1 placeholder to 0.1.0 on first
publish from the new home). document-canvas, knot-editor-host, and the three
surface-engine adapters follow the same pattern if/when publishing them is
useful; nothing forces it.

## Per-engine feature granularity

Apps choose engines as build conditions. Two layers:

- **nematic**: today all fifteen document engines compile unconditionally.
  Add per-format features (gemtext, gopher, nex, finger, spartan, guppy,
  titan, scroll, misfin, feed, file, markdown, knot, text) so an app can
  include gemini and exclude spartan. Default = everything, matching today.
- **inker engine adapters**: scrying/graft/weld are already separate crates;
  an app's manifest picks which to dep and register. Pelt's genet-lane
  engine impls follow pelt-desktop's existing feature ladder (tile-surface /
  scripted / scripted-nova / smolweb).

Example app postures (illustrative only): strophe takes nematic
markdown+file for docs panes; isometry takes nematic file+markdown plus
scrying for its compendium web cards; merecat takes everything with graft and
weld off by default.

## Mere-side consumer flips

gloss, uxtree, import, and meerkat switch `inker.workspace = true` to the
genet git dep (branch = main), the same pattern they already use for
pelt-desktop and genet-extract. The local-override loop stays the usual
gitignored `.cargo/config.toml` patch.

## Companion rehomes (consumer sweep, 2026-07-09)

Verified consumers across repos/:

- **errand** (smolweb transport + parse): nematic, meerkat, mere
  system/fetch, genet smolweb-views, pelt-desktop. After the inker move,
  three of five consumers are genet-side and the smolweb column (errand
  parse, nematic blocks, smolweb-views native views, pelt shell) lives in one
  repo. **Recommend: move into genet as a component.** errand keeps its
  MIT/Apache license per-crate. mere/merecat's fetch actor keeps consuming it
  via git dep. Its protocol deps (nex-protocol, spartan-protocol,
  guppy-protocol; each errand-only) come along as workspace members under
  errand's subtree, each keeping its own published crate identity
  (decision, Mark 2026-07-09: co-located is fine as long as they stay their
  own crates). Their standalone repos retire once the moves land. misfin is
  the exception: stewardship doctrine plus direct mere consumers
  (shell/comms) keep it a standalone repo.
- **illume** (decision, Mark 2026-07-09: **moves into genet**). Consumers:
  knot-editor-host, meerkat, genet root, xilem-serval. The general-purpose
  identity in its current README (host- and toolkit-agnostic lexer/
  highlighter, MIT/Apache) survives the move; it publishes from genet like
  the other adopted components. Rationale: the text stack consolidates in
  the render/host-framework repo, and xilem-serval already deps it. (The
  stale 0.0.1 "for the Mere browser" registry description gets rewritten on
  republish regardless.)
- **tinct** (repo repos/tincture; decision, Mark 2026-07-09: **moves into
  genet**). Perceptual OKLCH seed-to-palette derivation plus the
  contrast-gated syntax palette; 913 LOC, serde-only deps, 0.1.1, MIT/Apache.
  Consumers span both workspaces already: genet root, xilem-serval,
  strophe, plus mere/meerkat/register-theme. It is illume's designated
  palette partner, so the pair moves together.

  With knot-editor-host already in the main move table, this lands the whole
  text/editing column in genet: illume spans, tinct palettes,
  knot-editor-host editor model, xilem-serval text surfaces.

### verso family (amends the boundary-pass slate)

The 2026-07-09 boundary-pass plan slates verso to move with merecat, grouped
with orrery/platen. With inker in genet, that grouping looks stale: verso is
engine machinery, not app orchestration. It is the *dynamic* counterpart of
inker's multiplexer: inker picks an engine per address; verso swaps engines
mid-session (the compatibility-view flip), carrying cookies/scroll/forms from
a glass-box donor (verso-genet) to a black-box receiver (verso-scry).

Dependencies, verified 2026-07-09: verso-api is dependency-free by design;
verso and verso-scry dep only verso-api; verso-genet deps genet git crates
(genet-scripted-dom, layout-dom-api). Zero kernel anywhere. Moving the
family into genet removes verso-genet's cross-repo hop entirely. Consumers
(meerkat, system/fetch) flip to the genet git dep like inker's.

**Decision (Mark, 2026-07-09): the four crates consolidate into one genet
component named `verso-tile`**, the crates.io name the family already holds
(crates.io `verso` belongs to an unrelated literate-programming tool, so the
orchestrator could never publish under its local name anyway; consolidation
resolves the collision and the four-crate sprawl at once). This follows the
bundling rule for lockstep single-locus families: one crate, sub-modules.

Proposed shape (illustrative only):

    verso-tile/
      src/api.rs      (was verso-api: PortableViewState, donor/receiver traits)
      src/flip.rs     (was verso: the FlipDonor/FlipReceiver orchestrator)
      src/scry.rs     (was verso-scry: black-box receiver, ScrySurface seam)
      src/genet.rs   (was verso-genet: glass-box donor)

Feature layering preserves verso-api's charter (an external black-box
implementor must reach the contracts without engine deps): default features =
api + flip + scry, no engine dependencies; a `genet-donor` feature gates
`src/genet.rs` and its genet-scripted-dom + layout-dom-api deps. The
published verso-tile description is rewritten to the flip charter on next
publish. meerkat-browser-worker stays app-side (it is the app's worker host,
not flip machinery). The boundary-pass slate is amended in its own doc: verso
leaves the merecat web lane for genet; the first-vertical-path statement
(routes through verso-api from day one) survives with the import path
changed.

## Explicit non-moves

- **wgpu-scry / wgpu-weld / wgpu-graft, misfin**: standalone public libs by
  standing doctrine; crates.io-only, one-way deps. (misfin additionally has
  direct mere consumers: shell/comms.)
- **netrender, netfetcher**: the established sibling shape for engine-grade
  crates; both serve genet and mere/merecat equally.
- **muniment / codicil / chartulary / stemma / scholia**: the data family's
  extraction out of mere is fresh, deliberate design (G0-G5); not revisited
  here.
- **armillary, numen/quint/seiche, vates, sibylla, personae, retinue**:
  multi-workspace or deliberately layered siblings; unchanged.

## Done conditions

1. inker + nematic + document-canvas + knot-editor-host + three engine
   adapters build as genet workspace members; `cargo check` green across
   genet's default and per-feature builds. **DONE 2026-07-10** (genet
   af674dbad): nematic at components/nematic (top-level), the rest under
   components/inker; inker + nematic publish = true at 0.1.0 with
   repository = mark-ik/genet; document-canvas's netrender deps unified
   onto the workspace git pins (resolved question above); 314 family tests
   green + document-canvas --features netrender checks. Two test-only
   fixes for netrender's FontRecord Arc change (latent; libs were fine).
   nematic's "static HTML" doc rot fixed in the description on the way.
2. inker core has no kernel dependency; routing tests pass without kernel
   fixtures. **DONE 2026-07-10** (mere 7fde32b): NodeKey = petgraph
   NodeIndex direct, RouteViewId own uuid newtype in routing/ids.rs; every
   in-tree constructor passed view: None, so no call site changed; inker's
   tree is serde + tracing + petgraph + uuid.
3. statements apply-half relocated mere-side (or parked in a named follow-on
   if the linked-data plan has not landed a consumer). **DONE 2026-07-10**
   (same commit): resolve_rel + StatementOutcome + apply_link_statements
   now in linked-data (apply takes `&[LinkStatement]`, so linked-data deps
   inker only for the vocabulary); inker keeps the pure walk. Receipts:
   inker 86 / linked-data 35 / meerkat 82+247 green, all inker consumers
   check clean.
4. mere's gloss/uxtree/import/meerkat build against the genet git dep;
   meerkat behavior unchanged (existing routing + registry tests green).
   **DONE 2026-07-10** (mere 95b7391): crates/inker deleted; six
   workspace deps flipped to genet git (member crates' `workspace = true`
   lines untouched; knot-editor-host had no mere consumer, no entry);
   local .cargo/config.toml patches extended. gloss / uxtree / import /
   platen / content-contract / linked-data check green; meerkat 82+247.
   NOTE: mere's committed manifests now need genet commit af674dbad
   pushed to GitHub for a clean non-local build (the local patch file
   covers the working-copy loop meanwhile).
5. App-flavored routing constants live app-side; grep for
   `graphshell.internal` in genet returns nothing. **DONE 2026-07-10**
   (genet efeff274ba4, mere baee697): graphshell.internal +
   linked-data.ingest + is_graph_contribution_route + their default-policy
   rules moved to the new `mere::routing` module (facade grows an inker
   dep; `route_policy()` layers the app rules); meerkat's three production
   policy sites switch to it. Deviation, recorded: `host.external-protocol`
   STAYS in inker as the neutral hand-to-OS fallback the default policy's
   fallback rule needs — it is host-generic, not mere-flavored.
6. Pelt's genet lanes registered as inker document engines behind pelt
   features; meerkat's hand-dispatch of genet lanes retired or reduced to
   registry calls. **Superseded 2026-07-10**: widened into its own design
   pass, [2026-07-10_session_engines_plan.md](./2026-07-10_session_engines_plan.md)
   (a third engine kind — retained sessions producing paint frames — plus
   the genet-sessions component, the meerkat ladder collapse, and the
   errand/nematic reorg folded in). "Document engines" was the wrong shape
   for the genet lanes: forcing them through EngineDocument is the block
   mash already rejected.
7. crates.io: inker + nematic republished from mark-ik/genet.
   **PREPARED + PARTIALLY BLOCKED, 2026-07-11** (genet 66d88be5efa):
   versions bumped and package-verified for inker 0.1.0, illume 0.0.2,
   tinct 0.1.2, nex/spartan/guppy-protocol 0.1.1, then errand 0.1.3 (its
   =0.1.1 pins resolve once the protocols land — publish order matters).
   The publish itself awaits Mark's hand (permission boundary). nematic
   and verso-tile CANNOT publish yet: their optional path deps
   (xilem-serval, layout-dom-api/genet-scripted-dom) are unpublished
   genet components, and cargo requires every dependency versioned and
   registry-resolvable — their 0.0.1 name claims stand meanwhile.
   **Archive half DONE 2026-07-11**: errand, nex/spartan/guppy-protocol,
   illume, tincture all pushed-final, tombstone-described, archived on
   GitHub, each with a homepage link to its genet path (links resolve
   once genet is pushed).
8. errand joins genet (companion step; may land separately). **DONE
   2026-07-10** (genet e4487163ed0, mere 00cf124): components/errand with
   nex/spartan/guppy-protocol as members under protocols/, each keeping
   its published identity; genet consumers (nematic, smolweb-views,
   pelt-desktop) on workspace entries; mere's fetch + meerkat flipped to
   the genet source. errand's stale MPL SPDX comment (contradicting its
   MIT/Apache license field) dropped in transit. 93 tests green across
   the four crates.
9. verso-tile consolidated (four crates to one, feature-layered as above)
   and building as a genet component; meerkat and system/fetch on the git
   dep; boundary-pass doc amended. **DONE 2026-07-10**: landed exactly as
   specified (api/flip/scry modules unconditional and dependency-free,
   `genet-donor` feature gates the glass-box donor + its scripted-dom deps);
   all four crates' tests came along (12 default / 15 with the donor
   feature); the four mere crates deleted, meerkat + system/fetch flipped
   (meerkat's `engine-scry` feature no longer gates the scry *receiver*,
   which is dependency-free — only the platform producer deps stay gated);
   meerkat suite green. Note landed ahead of the inker move (the verso
   family never depped inker, so item 9 was independent).
10. illume + tinct adopted as genet components (companion; may land
    separately); their standalone repos retire; registry descriptions
    refreshed on republish. **DONE 2026-07-10** (genet c398e2b96a7, mere
    4e263a5): components/illume + components/tinct (repo was named
    tincture); root git/crates.io deps became path entries; mere's
    tincture alias flips to the genet source; xilem-serval's highlight
    feature checks clean; illume 32 + tinct 6 + knot-editor-host 15 tests
    green. Repo retirement (archiving repos/errand, repos/illume,
    repos/tincture, the three protocol repos) is Mark's call once genet
    is pushed.

## Open questions

- Statements apply-half: RESOLVED 2026-07-10 (Mark). Moves into mere's
  `crates/graph/linked-data` as part of the inker move.
- Smolweb lane unification: RESOLVED 2026-07-10 (Mark) against the
  fold-together candidate. Retargeting smolweb-views onto `DocumentBlock`
  was assessed and rejected: blocks are a *normalizing* reading vocabulary
  (canvas cards, LOD, AccessKit roles) and the lowering is structurally
  lossy. Two in-tree receipts: nematic's gopher engine collapses every
  non-info `GopherKind` (Submenu, Search, Binary, Image, Sound...) into an
  identical `link_paragraph`, so a type-7 search item loses the input
  affordance its idiom requires; and `Block::Preformatted { text }` carries
  no alt field, so gemtext preformat alt text dies at the block boundary.
  Rendering the native lane from blocks would be gemtext-to-HTML mashing
  with extra steps, the exact path smolweb-views was built to refuse
  ("Native, not gemtext-to-HTML"). Decision: per-format AST views are the
  idiom carriers. Native-lane coverage grows the honest way: wrapper
  protocols whose bodies ARE gemtext/markdown by definition (spartan,
  titan, misfin, guppy, scroll) route bodies through the existing
  gemtext/markdown views plus thin per-protocol chrome (status line, upload
  affordance, mail headers); genuinely distinct formats (nex, finger) get
  their own small AST views.

  REVISED same day (Mark): blocks do not keep even the card/summary lane by
  default. Historically `DocumentBlock` was knot's model: faithful embedding
  of protocol formats and executable-fence outputs inside a note (web clips,
  scripting languages; `script/rhai` producing EngineDocument is that
  purpose in code). It drifted into an app-wide lowest-common-denominator
  document model, and that drift is not wanted capping chisel card widgets.
  Direction: native views are prioritized everywhere protocol content
  renders, full documents and cards alike; blocks revert toward the
  authored/stored model (knot notes, clips, script outputs — content that
  must serialize). Knot's polyglot fences should render through the native
  views too, fulfilling the original fidelity intent instead of expanding
  through the lossy block engines.
- Errand/nematic concern reorg (follow-on design doc, named 2026-07-10):
  with both in genet, re-split the load to prioritize native views.
  Sketch: errand owns ALL wire parsing (nematic never touches bytes);
  nematic pivots from block-lowering to the native-view home, absorbing
  smolweb-views; block lowering shrinks to the consumers that genuinely
  need a serialized model. The deep cut to design honestly: inker's
  `Engine` trait returns `EngineDocument`, so demoting blocks from
  protocol rendering changes the document registry's contract for the
  smolweb IDs (views are not serializable; the trait's output shape or the
  dispatch path must account for a native-view lane). Block-consumer
  census (2026-07-10, mere-side): gloss, uxtree, platen document_scene
  (pane content as documents), meerkat cards/note surfaces/page_text/
  inspector, import web_clip, script/rhai. Sequencing: move first
  mechanically (this plan, everything stays green), reorg second once
  colocated in genet — do not redesign mid-move.
- document-canvas netrender pins: RESOLVED 2026-07-10 (Mark). Unify onto
  genet's existing workspace netrender entries during the move.
- knot's home: RESOLVED 2026-07-09. The knot column lands in genet
  wholesale (knot engines with nematic, knot-editor-host with illume +
  tinct); mere consumes its native note format back via the git dep.
- Nematic doc rot to fix on the move: lib.rs and the crates.io description
  still advertise "static HTML", but no HTML engine exists (HTML went to
  genet's lanes per the scope doctrine); rewrite on republish.
