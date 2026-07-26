# Frisket: one pane model, as a Cambium module

**Date:** 2026-07-24
**Status:** landed. `cambium::frisket` plus the Pelt retarget; Pelt's copy of the
view is deleted. See [What landed](#what-landed).

The family currently spells "a tree of resizable panes" four times, in four
repos. This ruling converges them on one implementation, named **frisket**,
living as a module in Cambium. The name comes from turnstone's crate, which
retires into it.

Two intents from Mark govern it. First: draw boundaries such that duplicate
implementations of the same idea resolve to one. Second, and it decided the
shape: **modularity, packaging, and publishing are three different decisions,
and this workspace had been collapsing them into one.** Modules are free. A
crate buys compile-enforced boundaries and costs version drift, patch tables,
and a name to defend. Publishing is a public commitment. Frisket wants the
first only.

## The four spellings today

| Where | What it contributes | Home |
|---|---|---|
| [`genet-host-api::tile`](../components/genet-host-api/tile.rs) | The contract: n-ary splits with fractional shares, tab-stacks with an active tile, `TilePath`, `TileEvent`, `DropTarget`/`Edge`, and the `apply` reducer | components (product-neutral) |
| [`cambium::split`](../components/cambium/cambium/src/split.rs) + [`tabs`](../components/cambium/cambium/src/tabs.rs) | The furniture: draggable ARIA divider with keyboard resize and geometry as state math; the tab strip | components (both landed on turnstone consumer-pull, July) |
| [`pelt/desktop/tile_surface.rs`](../ports/pelt/desktop/tile_surface.rs) + [`tile_shell.rs`](../ports/pelt/desktop/tile_shell.rs) | The renderer: `TileTree` to Cambium flex DOM, tab activation, drag lane, content rects for compositing | **a port** — unreachable by anything else |
| `turnstone:crates/frisket` | A savable pane tree: binary splits with one ratio, a leaf per pane, persistence, uxtree projection | **an app** |

Mere sits across the seam: `platen::tree_projection::tile_tree_from_plan`
produces a `TileTree` that, today, nothing renders. Turnstone imports
`genet-host-api` only to convert a `SplitAxis` and hand-rolls its pane views on
the Cambium furniture instead. So the contract has a producer with no consumer
on one side, and a renderer that cannot be reached on the other.

## The ruling

**Genet, not Mere.** Pelt needs panes and cannot depend on Mere without
inverting the stack, so anything Pelt and Turnstone both need must live at or
below Genet. The [port boundary](2026-07-24_pelt_port_boundary.md)'s CI
dependency-cone witness already enforces that direction: components may not
depend on packages below `ports/`. Lifting the surface out of Pelt is the
sanctioned direction of travel, and the witness keeps it honest afterwards.

**Tile's tree wins; frisket's does not travel.** The two trees are different
models, not two spellings of one:

- turnstone frisket: binary `Split { axis, ratio, first, second }`, one content
  per leaf, no tabs, `First`/`Second` paths.
- `genet-host-api::tile`: n-ary branches with fractional shares, tab-stacks
  with an active tile, `Vec<usize>` paths, a reducer, drop targets.

Tile is the more capable model and the one with a renderer. Landing frisket's
tree beside it would leave two tiling models in one repo, which is worse than
the status quo.

**Only the name travels.** turnstone's frisket is graph-bound by construction:
every `PaneNode::Leaf` carries a `GraphId`, the multi-graph rule lives in the
tree ("differing IDs in one frame = multi-graph window"), and four operations
exist to retag that binding. That is Mere's multi-graph pane model, and it must
not enter the repo whose contract says it "never grows toward forme". Turnstone's
convergence is to retire its tree onto frisket's and keep `PaneContent` plus
`graph_id` as **content payload** behind an open content lane, not as tree
structure.

> **Corrected 2026-07-25.** This paragraph's conclusion did not survive contact
> with turnstone's code: it composites one surface per pane rather than rendering a
> pane frame, so its tree is not a duplicate to retire. See
> [Follow-ons](#follow-ons-2026-07-25) §2.

## Shape

**A module in Cambium. No new crate, nothing published, nothing else moved.**

- `genet-host-api::tile` stays where it is: the contract, already
  zero-dependency, already what platen depends on.
- **`cambium::frisket`** is the surface, lifted out of Pelt's port: the
  tree-to-DOM mapping, tab activation, the divider and drag lanes. GPU and
  document compositing stay host-side.
- `cambium::split` and `cambium::tabs` become its siblings rather than its
  dependencies — the module composes furniture from the same crate.
- `turnstone:crates/frisket` is deleted. The name survives as the module's.

Why Cambium rather than a new component, in order of force:

1. **It is already the shared home.** Pelt, Turnstone, Woodshed, and Isometry all
   depend on Cambium. A new component would need a new dependency edge, a new
   version to keep aligned, and a new `[patch]` line in four repos — the exact
   tax that produced three silent-green build failures in this family on
   2026-07-24 alone (a pin that stopped matching its patch, an unpublishable
   crate forcing git deps, and a patch keyed to a retired source).
2. **It is the right altitude.** Cambium already holds compositions of this
   size: `graph_canvas`, `command_surface`, `overlay_surface`, `detail_panel`,
   `action_list`. A pane surface is not a bigger thing than a graph canvas.
3. **It resolves two open questions for free.** Cambium is MPL-2.0, and the
   lifted Pelt code is MPL, so no relicensing question arises. And the rect
   authority stops being a choice: an in-crate module must use `split`'s
   state-math geometry, because the alternative is drifting from a sibling.

The one new dependency edge is Cambium onto `genet-host-api` for the contract
types. Both are components, so the direction is legal, and the contract is a
zero-dependency leaf. Ordering constraint: Cambium is `publish = true` while
`genet-host-api` is `publish = true` but not yet on crates.io, so it has to go
up before or alongside Cambium's next release.

Wins on landing: Pelt keeps working through the extracted component, Turnstone
deletes its hand-rolled pane views and its own tree, platen's projection
finally reaches a renderer, and Woodshed and Isometry can have panes without
importing Mere.

## The name

A frisket is the hinged frame whose cut-out apertures decide what prints where,
which describes the Cambium module at least as well as it described the turnstone
crate. Mark: fun names are welcome; what is not warranted is announcing every
one of them to the world.

So the name lives on with no *consumable* package attached — but not as an
orphan. The first draft here ruled "orphan like eidetic", and Mark caught the
mismatch (2026-07-26): eidetic's bare name fell out of use, while frisket's is
the module's *active* name, so leaving crates.io describing "the pane model for
Turnstone" against a deleted crate would misinform about our own vocabulary. The
fix is genet's existing practice: a **name claim** (`support/name-claims/
frisket`, 0.0.2, the fourteenth in that directory) whose description points at
the real home — the `frisket` module of `cambium` 0.3.2+ — and whose repository
points at genet. Deliberately doc-only, no re-export of cambium: a facade would
be a second consumable path to the module (two names for one idea, the thing
this ruling exists to stop) and a version pin that goes stale.

The press metaphor does now straddle two repos, since forme and platen stay in
Mere. Accepted deliberately.

## Open decisions

Both were answered on 2026-07-25; see [Follow-ons](#follow-ons-2026-07-25) for
what landed. Kept here as the questions the ruling had to leave open:
the shape of `ContentSource`'s open tail, and reconciling Cambium `split`'s
state-math geometry with Pelt's flex-derived rects.

## Sequencing

**Landed 2026-07-25** rather than waiting for a payer, because Pelt was already
the consumer: lifting its surface out of the port is what created the module.
The other trigger named here, "Turnstone retargeting its panes", turned out to be
the wrong goal on inspection — see [Follow-ons](#follow-ons-2026-07-25) §2.
Isometry wanting tiling for its overmap, board, and compendium remains a live
future consumer; Woodshed needs a tool panel beside practice, which is one split
that Cambium's `split` already covers.

## What landed

**`cambium::frisket`** (7 tests): `frisket(&tree, on_event)` renders splits,
dividers, tab bars, and one content hole per stack. The tree stays the caller's
state and the component reports [`TileEvent`]s rather than mutating it. With it,
the DOM semantics that had been duplicated per host: `divider_target`,
`tab_target`, `stack_target`, `tab_drop_index`, `encode_pane_path` /
`decode_pane_path`, and `FRISKET_CSS`. Tests cover the DOM shape (N children
means N-1 dividers; the content hole names the *active* tile; fractions ride as
flex-grow), the ARIA posture (tab roles, `aria-selected` on the active tab only,
the divider as an oriented separator), gesture reporting (a close does not also
activate, and the tree is untouched), and each target lookup.

**No layout dependency was added.** Cambium builds views and does not lay them
out, so a host hit-tests its own layout and asks the component what it hit. The
one gesture needing real geometry, a tab drop's insertion index, takes rects
through a closure. That also settles the rect-authority question by
construction: the module cannot derive geometry independently, so there is
nothing to drift from `split`.

**Pelt retargeted**, which is what makes this a convergence rather than a fifth
spelling: `tile_surface.rs` lost 259 lines and gained 73 (its `render_node`,
`render_stack`, path codec, and frame CSS are gone), its three hit lanes now ask
the component what was hit and keep only the measurement that is genuinely
theirs (a split's pixel extent, for turning a drag delta into a fraction), and
`FRISKET_CSS` joins its sheet list rather than being forked into it. Pelt's 13
tile-surface tests pass unchanged in intent; the suite is 24 green, Cambium 149.

**The blocker, and why it was a pin rather than a publish.** Pelt could not see
the module because `ports/pelt/desktop` pinned `cambium = "0.2.0"` from the
registry while the workspace copy was 0.3.1. That pin was not a live decision:
it landed 2026-07-14 (`70e5205`, "Consume the published Cambium stack in Pelt"),
**nine days before Cambium became a Genet component** on 2026-07-23. It was the
only sane arrangement while Cambium was a sibling repo, and it outlived its
reason at the adoption, leaving the reference host building against a stale
published toolkit. Pelt now path-deps the workspace copy like every other
component in that manifest, so no publish is needed for in-repo work. Publishing
`genet-host-api` before Cambium's next release remains a real ordering
constraint for *external* consumers, but it no longer gates anything here.

Fourth instance in two days of one failure shape: a component separated from its
consumer by a version pin, each announcing itself only as a missing symbol or
cargo's "patch was not used". The others: Woodshed's stale `cambium = "0.2.0"`,
`cambium-winit`'s unpublishability, and `tinct`'s patch keyed to a retired repo.

## Follow-ons, 2026-07-25

### 1. `ContentSource` open tail: done

`ContentSource::Open { kind, id }` landed, the recognized-core-plus-open-tail
shape `sceno::Representation` uses. `kind` names the lane, namespaced like
`SettingsRef` (`"turnstone.roster"`, `"woodshed.fretboard"`); `id` is opaque here.
An unrecognized kind degrades to an empty pane, because a Genet surface draws the
frame and leaves the hole for its host either way. Test:
`an_open_lane_is_carried_not_interpreted` — a non-browser pane rides the tree and
the reducer verbatim and is never a `Document` wearing a costume. The three named
lanes remain the ones a Genet surface composites itself. genet-host-api: 13
tests green; Pelt unaffected (its matches carry catch-alls).

### 2. The Turnstone half: the goal was wrong, and the code says why

Retargeting turnstone's panes onto `cambium::frisket` **should not happen**, and
the doc above was written from an outside-in reading that does not survive
contact with turnstone.

Turnstone does not render a pane frame. `shell::surface_plan` walks its pane tree
into **one composited surface per pane** plus divider bands, which is how a live
WebView, the Orrery's GPU canvas, and a cambium-rendered pane coexist in one
window. `src/pane.rs` is that host-side geometry, and it already consumes
`cambium::Split`'s state math rather than its view — deliberately, since
2026-07-17: "turnstone composites surfaces, so it consumes the component's state
math rather than its view (the math is the single geometry truth)". `src/
cambium_pane.rs` is not a pane renderer either; it is the seam for rendering a
cambium view *inside* one turnstone pane surface.

So the two are not two spellings of one idea. They are two presentation
strategies — a DOM frame with content holes, and per-pane composited surfaces —
over one shared geometry truth that was already converged in July. What looked
like duplication from the outside was the shared part already being shared.

**Recorded as a deliberate divergence**, not debt: the family has two pane tree
shapes (turnstone's binary split with one ratio, the contract's n-ary branches with
tab-stacks). Converging them costs ~473 references and buys nothing while the
renderers differ on purpose. Revisit only if turnstone ever wants tabs, or wants
its panes inside one DOM.

**What did land:** turnstone's `frisket` crate folded into `src/panes` (2026-07-25).
It had exactly one consumer, so it wanted no package identity, and the name
`frisket` now belongs to `cambium::frisket`, which turnstone consumes — a crate and
a module of the same name in one dependency graph is a trap for the next reader.
13 call-site files rebased, the workspace member and path dep removed, `serde`
and `incipit` moved onto turnstone's own manifest. The published `frisket 0.0.1`
becomes an orphaned version, the end `eidetic`'s bare name reached.

### 3. Publish order: proven, and Mark's step

`cargo publish --dry-run` on both, so the order is measured rather than assumed:

- `genet-host-api 0.1.0` packages clean (6 files, 10.6KiB compressed). Manifest
  rounded out with keywords and categories for a first release.
- `cambium 0.3.2` **fails**: "no matching package named `genet-host-api` found".
  Cambium's new dependency on the contract is the only thing blocking it.

So the order was genet-host-api, then cambium. **Both published 2026-07-25**
(genet-host-api 0.1.0; cambium 0.3.2 with the host-api dep resolving on the
index), so the registry story is whole for out-of-family consumers. The family
itself is unchanged: Woodshed, Hocket, and Isometry take Cambium from
`genet.git` by branch, and Pelt path-deps the workspace copy — the git-only rule
for the cambium family still stands, since the registry sprigging/cambium pair
still carries the crates.io `paint_list_api` while the stack git-deps
netrender's.
