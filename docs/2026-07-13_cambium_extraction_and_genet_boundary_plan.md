# Cambium extraction and Genet boundary plan

**Date:** 2026-07-13
**Status:** in progress; extraction and native Cambium/Sprigging source naming
have landed across all five consumers. Three C4 verification walls pass; the
Woodshed and Strophe walls remain in Cargo dependency resolution.

## Decision

Create **Cambium**, a standalone Genet-native reactive GUI toolkit derived
from Xilem's reactive core and the current `serval-xilem` backend. Name the
reactive core **Meristem**: it is the structure-producing layer inside the
Cambium umbrella, while `cambium` remains the application-facing crate.

The correction that keeps the architecture honest:

- **Genet replaces Masonry.** Genet owns DOM, style, layout, paint, input,
  accessibility, and browser-engine behavior.
- **Sprigging extends Genet.** It supplies retained custom-paint leaves and
  arrangement geometry for pixels and virtualization CSS cannot express.
- **Cambium replaces the Xilem application layer.** It owns reactive diffing,
  application views, controls, component composition, and the host adapters
  which translate platform input into its event vocabulary.

The target dependency rule is strict:

```text
applications -> Cambium -> Genet seams -> netrender / platform
                         -> Nematic / other document engines as needed

Genet engine crates -X-> Cambium
```

Cambium is both a Genet consumer and the GUI provider to Mere, Isometry,
Strophe, and Woodshed. Genet remains independently usable as a browser and
document engine without Cambium.

**Naming migration:** Genet is the engine product formerly called Genet. The
repository, current `genet-*` packages, Rust identifiers, and historical source
references retain their Genet names until the source migration lands. Product
ownership statements in this plan use Genet; literal current identifiers keep
their existing spelling.

## Why extract now

The current placement no longer describes the code:

1. `components/xilem-core` is documented as a vendored, verbatim upstream
   mirror, but it now contains Cambium-specific `ElementSplice` extensions:
   `hoist_pending`, `extract_pending`, and `adopt_pending`. Those extensions
   power node-preserving same-parent and cross-parent movement. This is a fork,
   not a vendor snapshot.
2. `components/xilem-serval` is an application toolkit with controls, editors,
   overlays, component composition, multi-window projection, and portable keyed
   views. Its only natural relation to Genet is that it targets Genet's DOM.
3. `components/chisel`, now named Sprigging, is already consumed by Mere,
   Strophe, and Woodshed as a shared GUI substrate. Its catalog and lifecycle
   move with the application
   toolkit more naturally than with the browser engine.
4. Applications currently pin Genet `main` for the GUI crates. UI updates are
   therefore coupled to CSS-engine realignment and browser-engine churn even
   when their UI dependency did not change.
5. The reverse dependencies from Genet into `serval-xilem` are small and
   identifiable. Removing them produces a real one-way boundary.

## Source bases and namespace check

Record these before any history rewrite or directory move:

- Genet source: `6b955ff96ed8b2912d04f7a36a85a36b401bb780`.
- `mark-ik/xilem` main: `5d72ad41eb660fa620110e045d332fd95684ebae`.
- `linebender/xilem` main observed during the audit:
  `c5950bcb03d4f3d187a20d1159f6aa276fd056bf`.
- The crates.io names `cambium`, `meristem`, and `sprigging` were claimed by
  this project on 2026-07-13. The adjacent CSS names `genet` and
  `genet-stylo` were claimed at the same time.
- `cambium-winit` and `cambium-nematic` were unclaimed when audited on
  2026-07-13; that observation is not a reservation.
- `mark-ik/cambium` was not present on GitHub when this was written; it was
  **created public and pushed on 2026-07-13** (head `f2524901`, six commits:
  extraction, Sprigging relicense, Genet alignment, winit adapter, component
  catalog, Nematic views). Repository publication only — crates.io releases
  remain separately authorized.

The claims establish namespace ownership. Publishing implementation releases
remains a separately authorized action.

## Current ownership map

| Current code | Actual responsibility | Target home |
| --- | --- | --- |
| `components/xilem-core` | reactive diff/message core plus Genet-required move extensions | `meristem` |
| `components/xilem-serval` | Genet DOM backend, app runner, controls, component catalog | `cambium` |
| `components/chisel` | custom-paint leaf contract, retention, arrangements, glyphs | `sprigging` |
| `genet-winit-host` key/modifier mapping | winit events into Cambium key types | `cambium-winit` |
| `genet-winit-host` render/surface/a11y core | generic Genet/netrender presentation | stays in Genet |
| `genet-render` Sprigging convenience adapters | Cambium leaf registry into Genet's neutral leaf seams | Cambium integration module |
| `nematic::views` | Cambium views over Nematic ASTs | `cambium-nematic` |
| `genet-documents::smolweb` native-view runner | Cambium-authored smolweb document adapter | `cambium-nematic` |
| Nematic parsers and `EngineDocument` lowering | protocol-faithful document engine | stays with Nematic/Genet engine family |
| Inker and `document-canvas` | engine routing and document-to-PaintList lowering | stay outside Cambium |
| `knot-editor-host` lexer/model code | editor intelligence over existing parsers | stays outside Cambium; view adapter may move later |
| Pelt | Genet reference shell and integration consumer | stays in Genet, depends on Cambium where needed |

## Target Cambium workspace

```text
cambium/
  Cargo.toml
  README.md
  ARCHITECTURE.md
  LICENSES/
  crates/
    meristem/           reactive core derived from xilem_core
    cambium/            Genet backend, runner, controls, catalog
    sprigging/          leaf contract, registry, arrangements, glyphs
    cambium-winit/      winit -> Cambium input mapping
    cambium-nematic/    optional native views/adapters over Nematic formats
  examples/
    counter/
    component-catalog/
    sprigging-leaves/
    smolweb/
  docs/
    upstream-xilem.md
    genet-compatibility.md
```

`cambium` is the application-facing crate. Most applications should depend on
it plus `sprigging` only when they define or directly manage custom leaves.
The other crates are explicit seams, not separate products an application must
assemble manually.

## Upstream and provenance policy

Do not retain the whole Xilem monorepo with Masonry and `xilem_web` merely to
delete or ignore them on every update. Cambium is a new toolkit with Xilem
lineage, not an alternate build profile of upstream Xilem.

Bootstrap the repository from both relevant histories:

1. Filter the Xilem fork history to `xilem_core`, root attribution/license
   material, and the core's tests.
2. Import the Genet histories for `components/xilem-core`,
   `components/xilem-serval`, and `components/chisel` into their target paths.
3. Preserve Linebender copyright and Apache headers on inherited files.
4. Add an `upstream-xilem` remote and a short patch ledger naming every local
   divergence from upstream core. The initial semantic patch set is the three
   node-preserving `ElementSplice` operations and the keyed/portable consumers
   built on them.
5. Reconcile upstream core deliberately by tag or pinned commit. Do not merge
   the whole Xilem workspace into Cambium main.

The Genet-authored Cambium backend retains its MPL headers. Meristem retains
Xilem's Apache-2.0 headers. Sprigging is original engine-neutral code and was
explicitly relicensed by its author to MIT OR Apache-2.0 on 2026-07-13. The
repository records each crate's license texts and SPDX identifiers.

## Staging

### C0 - Freeze the boundary and create a local extraction workspace

Work locally first. No GitHub rename, repository creation, name publication, or
consumer push is part of this stage.

**Current status:** landed locally. The workspace, architecture rule, package
metadata, and provenance ledger are in place. The four Genet core commits, 113
filtered upstream Xilem core commits, and the backend/Sprigging path histories
are joined or replayed with authorship intact.

Actions:

- Record the source commits above in the extraction commit.
- Create the Cambium workspace skeleton and history imports.
- Add the target package names and explicit path dependencies.
- Add `ARCHITECTURE.md` with the one-way dependency rule.
- Add `upstream-xilem.md` with the upstream base and patch ledger.

Done when:

- `git log --follow` reaches the relevant Genet history for Cambium and
  Sprigging; Meristem's two-parent history graft reaches both filtered Xilem and
  Genet lineage with `git log --full-history`.
- The workspace contains no Masonry or `xilem_web` packages.
- `cargo metadata --no-deps` succeeds without a Genet checkout path override.
- No package has been published and no remote state has changed.

### C1 - Make `meristem` the honest core fork

Start from the live Genet copy, because it contains the move-preservation API
that the public Xilem fork does not.

**Current status:** landed locally. The package and library are named
`meristem`, the three `ElementSplice` extensions are retained, the upstream
contract tests are restored, and the keyed and portable semantic tests pass in
the extracted Cambium backend.

Actions:

- Rename package `serval-xilem-core` to `meristem`.
- Rename the library surface to `meristem` inside the Cambium workspace.
- Preserve the three defaulted `ElementSplice` operations.
- Move the keyed and portable-move tests that prove why those operations exist.
- Compare every other source difference with the pinned upstream base. Classify
  it as upstream drift, formatting-only drift, or a Cambium patch.

Compatibility tactic:

- During consumer migration, Cargo dependency aliases may continue exposing the
  crate as `xilem_core`. Source imports can move separately from package source.
- Cambium's own source and documentation use `meristem` immediately.

Done when:

- `cargo test -p meristem` passes.
- The patch ledger accounts for every semantic difference from upstream.
- Removing any one of the three move operations breaks a named Cambium test.
- No Meristem module depends on Genet, Sprigging, winit, or a renderer.

### C2 - Move the Genet backend and Sprigging

**Current status:** partial. The crates and their path-filtered histories now
live in the local Cambium workspace as `cambium` and `sprigging`. They build and
test against published Genet seams, and Sprigging's default feature tree has
no wgpu backend. The example/catalog carryover remains.

Actions:

- Move `components/xilem-serval` to `crates/cambium`.
- Move `components/chisel` to `crates/sprigging`.
- Rename the public packages to `cambium` and `sprigging`.
- Replace internal `xilem_core` dependencies/imports with `meristem`.
- Update module documentation: Genet is the engine; Sprigging is the custom-leaf
  extension; Cambium is the application authoring layer.
- Carry over the existing focused tests and first-pixels fixture references.
- Make Vello CPU-scene use non-default-featured so Path-A-only Sprigging consumers
  do not pull the full wgpu backend stack.

Do not inflate Sprigging into a Masonry-shaped second engine. Its current leaf,
retention, arrangement, and glyph responsibilities remain the boundary.

Done when:

- `cargo test -p meristem -p cambium -p sprigging` passes.
- `cargo tree -p sprigging -e features` contains no wgpu backend for the
  default Path-A build.
- The component-catalog examples render using Genet as the sole layout/paint/
  input/accessibility engine.
- Cambium depends on versioned or git-addressable Genet seam crates, not paths
  back into the source checkout.

### C3 - Break every Genet -> Cambium edge

This is the extraction's decisive stage. Moving directories without this stage
would leave one distributed toolkit with a new name.

#### C3a - Window input

**Status (2026-07-13): landed.** `cambium-winit` owns key/modifier
translation. `genet-winit-host` now builds and tests without a Cambium
dependency. Pelt keeps a temporary app-side compatibility adapter until C4 can
consume the extracted crate from a stable source.

- Move `key_event_from_winit` and `modifiers_from_winit` from
  `genet-winit-host` to `cambium-winit`.
- `cambium-winit` depends on winit and Cambium. Applications compose it beside
  `genet-winit-host`; it does not wrap or depend on that presentation crate.
- Keep `RenderCore`, `WindowSurface`, `SurfaceHost`, wheel normalization, and the
  generic AccessKit bridge in Genet.

Done when `genet-winit-host` has no Cambium dependency and its tests still
cover presentation, wheel normalization, and accessibility bridging.

#### C3b - Sprigging render and accessibility adapters

**Status (2026-07-13): landed.** The engine protocol is now
`<custom-leaf>`/`custom_leaf_key`/`custom_leaf_boxes`; the old tag and accessor
remain compatibility aliases. Cambium emits the neutral tag. The remaining gap
was closed by moving `RenderedLeaves` assembly into Pelt and replacing
`genet-render`'s Cambium-driven tests with direct DOM/render regressions.
`genet-render` now has neither normal nor dev Cambium/Sprigging edges.

- Keep Genet's neutral `LeafPaintSource` and `LeafA11ySource` contracts.
- Neutralize the engine's current Chisel-specific DOM vocabulary. Rename the
  internal `<chisel-leaf>`/`chisel_leaf_key`/`chisel_leaf_boxes` seam to the
  working names `<custom-leaf>`/`custom_leaf_key`/`custom_leaf_boxes`, owned by
  Genet. Cambium and Sprigging stamp and consume that protocol. Keep the old tag as
  a migration alias only if a live external producer requires it.
- Move `RenderedLeaves` adapters and `*_with_leaves` convenience assembly into
  Cambium, or make the Genet entry points fully generic over the neutral source.
- Move `genet-render` tests that construct a `GenetAppRunner` into Cambium
  integration tests. Genet-render tests should construct DOMs directly.

Done when `genet-render` and `genet-layout` have no normal or dev dependency
on a Cambium crate, and their current code/docs do not name Sprigging as the owner
of the generic leaf seam.

#### C3c - Nematic views and smolweb document adapter

**Status (2026-07-13): landed.** `cambium-nematic` owns the four native AST
projections, their themes and tests, and a fetch-free retained document adapter
behind its `document` feature. Nematic's compatibility view module and Cambium
dependency are removed. `genet-documents` now retains an `EngineDocument`,
windows it through `document-canvas`, and lowers that PaintList to a scene while
preserving Pelt and Mere's theme, scroll, link-table, and navigation API. Its
smolweb dependency tree contains no Xilem, Cambium, Meristem, or Sprigging
package.

- Keep Nematic's AST parsing and `EngineDocument` lowering in Nematic.
- Move `nematic::views`, its view tests, and the view-driven
  `SmolwebDocument` into `cambium-nematic`.
- Split fetching from the moved document adapter. `cambium-nematic` accepts
  parsed ASTs or fetched bytes plus an address; it does not depend on Pelt's
  `ResourceFetcher`. The application/host continues to own transport.
- Keep the engine-native pipeline explicit:

  ```text
  fetch -> Inker route -> Nematic engine -> EngineDocument
        -> document-canvas -> PaintList -> netrender / host
  ```

- If `genet-documents` retains a smolweb feature, make it consume that
  engine-native pipeline. The Cambium-native DOM view is a downstream option,
  not the definition of a Genet document session.

Done when Nematic and `genet-documents` build without Cambium, while
`cambium-nematic` proves focusable links, theming, scrolling, and navigation
without a dependency on Pelt.

### C4 - Migrate consumers without a source flag day

**Status (2026-07-14): in progress, source migration complete.** Pelt,
Isometry, Woodshed, Strophe, and Mere now name Cambium and Sprigging in their
manifests and Rust imports. Pelt passes all 29 tile tests, Isometry passes all 20
`isometry-views` tests, and `scripts/check-meerkat.ps1` passes. The Meerkat wall
also moved winit key translation to `cambium-winit` and exposed two source
identity bugs: Sprigging's `paint_list_api` now follows Netrender's git source,
while Mere and Cambium share the published `tinct` package.

Woodshed and Strophe both parse after the rename, but their focused tests still
enter prolonged CPU-bound Cargo dependency resolution before compilation. A
fresh Strophe lock and a current Mere lock used as a donor produce the same
result, so stale lock content is not the sufficient cause. Isometry's refreshed
lock also contains its concurrent Stylo realignment and remains uncommitted;
the migration commits were isolated from that work.

First switch package source while preserving old Rust import names through
Cargo dependency aliases. Rename source imports to `cambium` and `sprigging`
in a separate commit per consumer.

Order:

1. Pelt, as the reference integration surface inside the Genet checkout.
2. Isometry, which consumes the view toolkit without direct Sprigging management.
3. Woodshed, which exercises Cambium plus `GraphGlyph` leaves.
4. Strophe, which exercises custom waveform leaves, meters, and leaf a11y.
5. Mere/meerkat, which exercises the widest control, highlight, multi-window,
   portable-keyed, menu, and Sprigging surface.

Per-consumer done condition:

- The lockfile contains Cambium packages from one intended source.
- The consumer no longer declares `serval-xilem`, `serval-xilem-core`, or
  `serval-chisel` as a direct dependency.
- Existing focused application tests pass.
- No application behavior is replaced with a placeholder during migration.

Focused verification commands:

```text
Pelt:      cargo test -p pelt-desktop --features tiles
Isometry:  cargo test -p isometry-views
Woodshed:  cargo test -p woodshed-core
           cargo build -p woodshed-genet
Strophe:   cargo test -p strophe-genet
Mere:      scripts/meerkat.ps1 check
```

Adjust a command only when the live package name differs; record the replacement
in the migration commit rather than weakening the verification wall.

### C5 - Publish or pin, then remove the Genet copies

Publishing is separately authorized external work. Until then, consumers may
pin a Cambium git commit or branch.

Before removing the originals:

- All five consumers pass C4.
- Genet's workspace passes without the three moved directories.
- Cambium's Genet compatibility table names the exact supported Genet package
  versions or git commit.
- Decide whether the already-published `serval-xilem-core`, `serval-xilem`, and
  `serval-chisel` packages need one thin compatibility release. Use actual
  external-consumer evidence; do not preserve wrappers indefinitely by default.

Then:

- Remove the moved workspace members and path dependencies from Genet.
- Replace old plan status text with explicit supersession links to Cambium.
- Keep historical implementation docs, but label their old crate homes as
  historical.
- Update application docs to say Cambium authors the UI and Genet renders it.

Done when:

- `rg "serval-xilem|serval-xilem-core|serval-chisel|xilem_serval"` finds only
  historical/supersession text in Genet and migrated application repositories.
- No Genet engine package has Cambium in its normal or dev dependency closure.
- A clean Cambium checkout can run its tests against the declared Genet source.
- A clean Genet checkout can run its engine tests without a Cambium checkout.

## Verification walls

### Dependency direction

For every Genet engine package, `cargo tree` must show no Cambium package.
Pelt may depend on Cambium because it is a reference host, not an engine crate.

Minimum audit set:

```text
cargo tree -p genet-layout
cargo tree -p genet-render
cargo tree -p genet-winit-host
cargo tree -p genet-documents
cargo tree -p nematic
```

### Cambium contract

```text
cargo test -p meristem
cargo test -p cambium
cargo test -p sprigging
cargo test -p cambium-winit
cargo test -p cambium-nematic
cargo metadata --all-features --no-deps
```

### Upstream drift

Every Meristem update records:

- upstream Xilem commit or tag;
- local patch ledger before and after;
- API changes required in Cambium;
- focused keyed/portable-move test results.

Never rebase Cambium blindly onto Xilem `main`. Prefer releases or a recorded
commit, then reconcile the small retained core surface.

## Risks and stop rules

### Hidden repository cycle

**Risk:** a convenience dependency leaves Genet depending on Cambium while
Cambium depends on Genet.

**Stop rule:** do not delete the Genet copies or publish Cambium until the C3
dependency audit is clean for normal and dev dependencies.

### Sprigging becomes a second GUI engine

**Risk:** “replace Masonry with Sprigging” grows Sprigging toward its own layout,
focus, input, accessibility tree, or window runtime.

**Stop rule:** child layout, hit-testing, focus, accessibility publication, and
window presentation remain Genet responsibilities. Sprigging contributes leaf
paint/state and arrangement coordinates only.

### Full-fork maintenance burden

**Risk:** retaining unused Masonry and web packages makes every upstream sync a
workspace-wide reconciliation.

**Stop rule:** Cambium main contains the reactive core and Cambium products, not
the whole Xilem monorepo.

### False package compatibility

**Risk:** package aliases make the build green while public docs and types still
claim the old ownership.

**Stop rule:** aliases are migration tools. Each consumer has a later source-
rename commit and the final grep wall admits only historical references.

### Mixed license assumptions

**Risk:** moving files is mistaken for changing their licenses.

**Stop rule:** preserve inherited headers and verify each package's declared
license against its shipped license texts before publication.

## Explicit non-goals

- Reimplementing Xilem's reactive core from scratch.
- Porting Masonry widgets or keeping Masonry as a hidden fallback.
- Preserving `xilem_web` as a Cambium backend. Cambium is Genet-native.
- Moving Inker, document-canvas, Nematic's engines, or Genet's renderer into
  Cambium merely because Cambium consumes them.
- Redesigning the Sprigging trait during extraction. Its partial layout/input
  contract is a later Cambium 0.2 decision after ownership is stable.
- Publishing crates, creating repositories, renaming GitHub remotes, or pushing
  consumer changes without explicit authorization.

## Completion definition

Cambium is extracted when all of the following are true:

1. Cambium owns the reactive core fork, Genet backend, Sprigging, component
   catalog, and platform-to-Cambium input adapters.
2. Genet owns the browser/document engine and exposes only neutral seams upward.
3. Dependency direction is one-way from Cambium to Genet for engine packages.
4. Pelt, Isometry, Woodshed, Strophe, and Mere consume Cambium successfully.
5. Xilem provenance and the Meristem patch ledger are explicit.
6. Old Genet package names are removed or intentionally compatibility-only.
7. Clean checkouts of Genet and Cambium verify independently against their
   declared dependencies.
