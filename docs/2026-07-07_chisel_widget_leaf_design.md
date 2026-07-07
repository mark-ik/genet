# chisel: custom-paint widget leaves for the serval host

**Status (2026-07-07):** proposed; first design pass. Defines a small, sharp
widget-leaf layer that lets imperative custom-paint widgets (knobs, meters,
waveforms, graph canvases) live as first-class serval elements without a second
UI engine. Sits beside the reactive backend from
[2026-05-27_serval_as_host_xilem_serval_plan.md](./2026-05-27_serval_as_host_xilem_serval_plan.md)
and rides the paint seams from
[2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md).
Working crate name **`chisel`** (needs a crates.io check before reservation).

Code samples are **illustrative** unless marked implementation-ready.

**Implementation status (2026-07-07):**

- `components/chisel` scaffolded and building (workspace member; deps
  `paint_list_api` + `accesskit`). Ships the `Leaf` trait, `PaintCx` (Path A),
  `LeafRegistry<K>`, and a `Swatch` leaf; a unit test drives measure → emit one
  `DrawRect` → `paint_dirty` clears (green).
- **Path-A layout seam wired in serval-layout (additive, green).** `<chisel-leaf
  key="…">` is a replaced element (`construct::is_replaced` +
  `chisel_leaf_key_of`), the key rides onto the box node (`box_tree`), and
  `paint_emit` splices the leaf's commands via a new `LeafPaintSource` trait +
  `emit_paint_list_with_leaves` entry point (existing entry points forward through
  an unchanged `emit_inner`, so no call site moved). An end-to-end test parses a
  `<chisel-leaf>`, lays it out, and asserts the source's command lands in the
  `ServalPaintList` (`paint_emit::tests::chisel_leaf_splices_its_path_a_commands`).
- **Leaf-tier retention cache (the new gate).** `RenderedLeaves` +
  `LeafRegistry::render_into` re-render a leaf only when it is `paint_dirty`, is
  uncached, **or its box size changed** since the cached buffer was painted (the
  size check catches container-driven relayouts that do not dirty the leaf).
  Tested (`render_into_respects_the_paint_dirty_gate`).
- **Coordinate model:** a Path-A leaf paints in its **content box**, `(0,0)` at
  the content-box origin, matching `<img>` / `<external-texture>`. `paint_emit`
  wraps the splice in a `PushTransform` at `content_offset` (border + padding);
  guarded by `chisel_leaf_offsets_commands_into_its_content_box`.
- **Adversarial review (2026-07-07):** two real bugs found and fixed (stale-sized
  cache; content-offset ignored) plus the emit branch chained after the
  image/external-texture branches (one replaced payload per box); the
  additive/byte-identical and engine-neutral claims were independently confirmed.
- **Author view landed.** `chisel_leaf(key, w, h)` in `xilem-serval`'s `tags.rs`
  builds the `<chisel-leaf key="…">` block box, exactly mirroring
  `external_texture`: the view carries only the stable key + a box, and the host
  registers the `Leaf` under that key out of band (as a texture producer registers
  a texture). Auto-exported via `pub use tags::*`; test
  `tags::tests::chisel_leaf_builds_keyed_element`.
- **Next (blocked on concurrent in-flight files — precise resume point):**
  1. In `serval-layout/lib.rs`, re-export `emit_paint_list_with_leaves` and
     `LeafPaintSource` (the explicit `pub use paint_emit::{…}` list does not carry
     them yet, so no external crate can call them). *Blocked: `lib.rs` is under
     concurrent edit.*
  2. Add a serval-layout helper to enumerate `<chisel-leaf>` nodes as
     `(u64 key, content-box Size)` from a laid-out doc (border/padding from the
     box node's `final_layout`), and re-export it. *Blocked: same `lib.rs`.*
  3. In `serval-render`, add a `_with_leaves` render entry (mirroring
     `paint_list_from_scripted_dom`) + a `RenderedLeaves` → `LeafPaintSource`
     newtype (orphan-rule-legal only here): cascade → layout → enumerate →
     `LeafRegistry::render_into` at content-box sizes → `emit_paint_list_with_leaves`.
     Add a `chisel` dep. Needs step 1/2's re-exports.
  4. Wire the call site + `LeafRegistry` ownership in `xilem-serval/runner.rs`.
     *Blocked: `runner.rs` is under concurrent edit.*
  5. On-screen smoke; then the orrery Path-B port.

  Steps 1-2 and 4 all edit files currently dirty with concurrent work, so the
  render glue waits for those to land (or for a greenlight to edit them in place).

---

## Thesis

The Linebender GUI stack is `xilem` (reactive) over `masonry` (retained widget
tree, own layout / paint / event / a11y passes) over `vello` / `parley` /
`accesskit` (substrate). serval already provides the middle and the substrate:
`ScriptedDom` is the retained tree, serval-layout runs cascade + Taffy + paint
emit, the host routes hit-tests, and serval-layout emits an accesskit tree.
netrender bottoms out in the same vello, and both stacks shape text with parley.

So we do not port Masonry. We keep Xilem's authoring idiom (already reused as
`xilem-serval`, the third `xilem_core` backend) and add one small thing: a
contract for a **leaf** that plugs into serval's four existing passes and paints
something the CSS vocabulary cannot say. That contract is `chisel`. It borrows
Masonry's four-pass idiom, not its code.

The payoff line: there is one instance of each pass (serval's), already running
every frame. A chisel leaf contributes one node's worth to each. No second
engine.

## Non-goals

chisel is deliberately narrow. It is **not**:

- **A Masonry port.** No retained widget tree, no `BoxConstraints` layout
  recursion, no Masonry passes. serval owns all of that.
- **The form-control catalog.** `button`, `checkbox`, `text_field`, `slider`,
  `select`, `radio_group`, `textarea` already exist as native `xilem-serval`
  views emitting common `PaintCmd`s. Anything the paint vocabulary can express
  stays a native view, not a chisel leaf.
- **A vello escape hatch by default.** A leaf whose paint decomposes to
  vocabulary primitives should emit them (Path A below) and stay portable and
  tile-cached. The texture path (Path B) is for paint the vocabulary genuinely
  cannot describe.

chisel is for the imperative / drawn minority, plus a uniform contract that
replaces today's ad-hoc custom-surface wiring (the hardcoded
`ORRERY_SCENE_KEY` / `GLOSS_MINIMAP_SCENE_KEY` branches in meerkat's
`render/compose.rs`, which the code itself notes lack a general registry).

## The two paint paths

A leaf paints one of two ways, chosen per leaf:

- **Path A — vocabulary.** The leaf pushes common `PaintCmd`s (`DrawPath`,
  `DrawLine`, gradients, `DrawText`) into the paint list at its own position in
  paint order. Result: resolution-independent, tile-cached, transportable across
  any backend, and correct under clip and z-order for free. Preferred for
  vector-shaped widgets (meters, waveforms, knobs, automation curves, graph
  edges). Note: this reuses the widget's *visual*, not Masonry's `paint()` code.
- **Path B — external texture.** The leaf renders its own `vello::Scene` (or
  wgpu output) which the host rasterizes to a `wgpu::Texture`, installs via
  netrender's existing `install_external_texture(key, texture)`, and places via
  `PaintCmd::DrawExternalTexture { texture_key: key }`. Per PM-3's lowering
  (`scene_op_boundary` in the compositor pass), external textures sit at any
  z-position in painter order, mid-page, not just topmost. Result: the widget's
  imperative paint code runs verbatim, at the cost of one texture. Preferred for
  shader effects, video, live 3D, anything not sayable in the vocabulary. serval
  already has `<external-texture>` as a replaced element on exactly this path
  (WebGL canvas uses it), so Path B is reuse, not new renderer machinery.

Both paths target the neutral seam crates, so a leaf never depends on the serval
engine concretely.

## The `Leaf` trait (illustrative)

```rust
// chisel — engine-neutral, against the seam crates.
pub trait Leaf {
    /// Intrinsic sizing, wired to serval's Taffy measure fn for this node
    /// (a chisel leaf is a replaced element, like an image, that measures
    /// itself). Returning a different size than last frame requests relayout
    /// through serval's ordinary IncrementalLayout path.
    fn measure(&mut self, known: SizeHint, available: SizeHint) -> Size;

    /// Paint. `PaintCx` exposes both flavors; the leaf uses one:
    ///   Path A: `cx.emit(cmd)` pushes a common PaintCmd.
    ///   Path B: `cx.scene()` hands out a vello::Scene the host rasterizes.
    fn paint(&mut self, cx: &mut PaintCx);

    /// Input. serval hit-test lands on this node and forwards here. Internal
    /// interaction mutates `self` and marks paint-dirty (no reactive round
    /// trip); a semantic change returns an action that xilem-serval routes up
    /// the message cycle (the Masonry widget/app split).
    fn event(&mut self, ev: &LeafEvent) -> Option<LeafAction>;

    /// Semantics. Fill this node's accesskit node during serval-layout's
    /// accesskit_tree walk (a knob still announces as a slider).
    fn accessibility(&mut self, node: &mut accesskit::Node);

    /// Retention gates. Two signals, because they gate different passes:
    ///   `paint_dirty` gates the repaint gate (has paint output changed?);
    ///   `layout_dirty` gates serval's relayout (has intrinsic size or, for an
    ///   arrangement leaf, child placement changed?). An interaction that only
    ///   redraws sets paint-dirty; one that resizes sets both.
    fn paint_dirty(&self) -> bool;
    fn layout_dirty(&self) -> bool;
}
```

Prop application is per-leaf, not on the trait: the `xilem-serval` view wrapper
for a concrete leaf type `L` holds `L`'s data, diffs it on rebuild, and pushes
changes into the retained leaf (the `WidgetMut` analog). The trait stays minimal:
the four passes plus one dirty query.

## Where retained state lives

Xilem's model is reactive diffing over *retained* widgets. In serval the
retained tree is the DOM, and DOM nodes are uniform `NodeId`s we do not want to
fatten. So a leaf's retained struct lives neither in the view (ephemeral,
rebuilt each update) nor in the DOM node. It lives in a **node-keyed registry
the host owns**, exactly like serval's existing `ExternalTextureRegistry` /
font / image registries:

```text
LeafRegistry: NodeId -> { leaf: Box<dyn Leaf>, cached_paint: PaintCache }
```

This is the architectural translation of "retained widgets": the DOM stays
uniform, widget state sits in a side-table keyed by node, and the reactive diff
writes props into the entry rather than rebuilding it. It mirrors the pattern
serval already trusts for external textures.

## Retention: four gates, one per pass

The core mechanism that "preempts recomputation" is not one cache. It is four
gates, each keyed by its own inputs, that compose so an unchanged subtree falls
through all four untouched. Three already exist; chisel adds one.

| Gate | Skips | Keyed on | Owner | Status |
| --- | --- | --- | --- | --- |
| `memoize` | view rebuild + diff | data equality | `xilem_core` | exists (re-exported in xilem-serval) |
| retained DOM + `IncrementalLayout` | relayout | which nodes mutated | serval-layout | exists |
| leaf dirty flag + `PaintCache` | repaint | widget-declared staleness | **chisel** | **new** |
| netrender tile cache | rasterization | SceneOp content hash | netrender | exists |

The frame loop:

1. App state changes. `xilem_core` rebuilds views; `memoize` short-circuits
   every subtree whose data is unchanged. Unchanged leaves are never visited.
2. For a leaf whose data did change, the diff does not rebuild the widget; it
   writes new props into the `LeafRegistry` entry and the leaf flips
   `paint_dirty`, plus `layout_dirty` if the change also affects its size or (for
   an arrangement leaf) child placement.
3. serval relayouts only mutated nodes. The leaf's measure fn (and, for an
   arrangement leaf, its child placement) re-runs only when `layout_dirty` is set.
4. At paint, serval consults `paint_dirty`. Clean reuses the cached texture
   (Path B) or cached commands (Path A); no `paint()` call. Dirty repaints, and
   even then the tile cache may reuse rasterization if the content hash is
   unchanged.

The only new machinery is the third gate. That is why chisel is small: it plugs
into three retention mechanisms that already run and adds a single leaf-owned
dirty bit plus a cache.

## Integration seams

How a chisel leaf attaches to each of serval's four passes:

1. **Layout.** A *paint leaf* is a childless **replaced element** (the same
   machinery as `<img>` and `<external-texture>`) whose intrinsic size feeds
   `construct::replaced_intrinsic_size` from `Leaf::measure`; serval owns the
   box, the leaf only contributes an intrinsic size. An *arrangement leaf* owns
   child placement; see [Arrangement leaves](#arrangement-leaves) below.
2. **Paint.** During `paint_emit`, a leaf node either splices its Path-A
   `PaintCmd`s into the stream at its position, or emits a
   `DrawExternalTexture` referencing the host-rasterized texture from
   `Leaf::paint`.
3. **Input.** serval's existing hit-test + `dispatch_click` / `dispatch_key`
   ancestor walk lands on the leaf node and forwards to `Leaf::event`. Returned
   actions ride the faithful `xilem_core` message cycle up to the app.
4. **Semantics.** serval-layout's `accesskit_tree` walk consults the
   `LeafRegistry` for leaf nodes and lets `Leaf::accessibility` fill the node.

## Arrangement leaves

A leaf that owns a private layout algorithm keeps its children **first-class to
serval** rather than sealing them inside an opaque box. It does this by placing
them, not by running a second layout engine:

- Children are real serval nodes (native views or nested leaves), created under
  the leaf node with `position: absolute`.
- The leaf computes each child's offset and writes it as the child's absolute
  `left` / `top` computed values. serval measures the children's own boxes; the
  leaf reads those measures and places. One layout engine, custom arrangement.
- **Z-awareness is native.** The leaf assigns each child a stacking value
  (`z-index`), and serval-layout's existing `paint_stacking` (CSS 2.1 Appendix E
  ordering) interleaves them correctly against each other, against the leaf's own
  Path-A paint, and against a Path-B texture via the `scene_op_boundary`
  ordered-composite path. chisel never runs a private compositor; it only
  chooses z, and serval orders. This is the "z-level awareness / compatibility"
  requirement: a chisel container composes into the same stacking model as every
  other serval node, so it nests inside, and contains, ordinary DOM without a
  seam.

The result keeps hit-test, accessibility, and paint working per child for free,
because each child is an ordinary serval node. What the leaf owns is arrangement
(x / y / z), nothing else.

The known hard edge is **incremental-layout reconciliation.** Because placement
runs in the leaf rather than in Taffy flow, a child resize has to flow back
through the leaf's arrangement and re-mark `layout_dirty` so serval reflows the
affected absolute offsets. The first cut may re-place all children on any child
change; a dirty-tracked subset is a later refinement. This is the one place the
one-engine story costs bookkeeping, and it is worth naming up front.

## Crate and repo structure

```text
paint_list_api   (shared seam)   PaintCmd, ExternalTextureItem, ...
layout_dom_api   (shared seam)   LayoutDomMut, NodeId, QualName
accesskit / vello / kurbo / peniko   (crates.io)

chisel                            Leaf, PaintCx, LeafRegistry, RenderedLeaves, catalog
    depends on: paint_list_api + accesskit (Path A today). vello/kurbo/peniko
                join when Path B (own vello::Scene) lands. No layout_dom_api
                needed: the registry is generic over the host's leaf key (u64).
    depends on: NOT xilem, NOT masonry, NOT serval-engine

xilem-serval                      reactive backend (stays a serval component)
    depends on: xilem_core (tracked branch), serval_scripted_dom,
                layout_dom_api, chisel (to expose leaves as views)
```

Decisions:

- **chisel starts as a serval component** (`components/chisel`), engine-neutral
  against the seam crates. It is a *sibling* of serval-layout (both target the
  seams), not a downstream of the engine.
- **Spinning chisel to its own repo is gated on the seam crates becoming
  standalone-consumable.** A chisel repo can only be as one-way and standalone as
  `paint_list_api` and `layout_dom_api` are. Until those are consumable outside
  the serval workspace, a repo would carry serval as a workspace dep and have the
  wgpu-sibling name without the wgpu-sibling one-way property. Revisit when the
  seams are extractable and chisel's contract has stopped churning.
- **xilem-serval does not spin out.** It consumes `ScriptedDom` concretely, so it
  is serval-downstream, not a sibling; a repo would invert the one-way direction.
  It is also still landing features (Stages 0-7). A later fork could genericize it
  over `layout_dom_api::LayoutDomMut` to make it a standalone "xilem_core backend
  for any LayoutDom engine," but that trades away its current "one node type, no
  type erasure" simplification, so it is a deliberate decision, not a rename. Do
  not name a spinout that should not happen yet.

Naming: `chisel` sits in the craft-tool cluster with the standalone wgpu
siblings (graft joins, weld joins, scry sees, chisel carves). That harmony is a
point in its favor; confirm the crates.io name is free (and only unreserved-held,
per usual practice) before committing.

## Building the catalog

Two tiers, and the catalog grows mostly on the native tier:

1. **Native `xilem-serval` views** for everything the vocabulary can say. This
   is most of a UI and is already begun.
2. **chisel leaves** for the imperative / drawn minority, each paired with a
   thin `xilem-serval` view wrapper (mirroring how `button` / `slider` are view
   functions today). Prefer Path A per leaf; reach for Path B only when the
   vocabulary cannot express the paint.

First catalog targets (done-conditions, not a schedule):

- A `Leaf` + `PaintCx` + `LeafRegistry` scaffold that renders one trivial
  Path-A leaf (a filled path) as a serval element, laid out by Taffy, with a
  passing headed smoke.
- One Path-B leaf (an arbitrary `vello::Scene`) composited via
  `DrawExternalTexture` at a mid-page z-position, proving the ordered-composite
  path end to end.
- The `paint_dirty` gate demonstrated: an unchanged leaf under a `memoize`d view
  produces zero `paint()` calls across frames (assert in a test).
- meerkat's orrery surface re-expressed as a chisel leaf, retiring one hardcoded
  `*_SCENE_KEY` branch in `compose.rs`, as the first real consumer.

## Seam verification (2026-07-07)

Read against serval's actual code. All four attachment points exist; one framing
corrected, one open question closed.

1. **Layout seam — confirmed, reframed.** The intrinsic-size hook is the
   **replaced-element** path (`construct::is_replaced` / `replaced_intrinsic_size`
   / `replaced_px_size`), not a raw Taffy `MeasureFunc`. This is stronger than the
   first draft claimed: `is_replaced` *already* returns true for
   `<external-texture>` ("a host-composited replaced element ... sizes like the
   default-object replaced," 300×150 default object size). So a Path-B leaf is
   the `<external-texture>` replaced element that already exists; a Path-A leaf is
   a new replaced kind whose paint emits `PaintCmd`s instead of a texture.
2. **Paint seam — confirmed.** `paint_emit` walks the DOM in paint order and
   already emits `DrawExternalTexture` for external-texture elements. Path A adds
   one emit branch (splice the leaf's cached `Vec<PaintCmd>` from the
   `LeafRegistry`); Path B reuses the existing external-texture emit.
3. **Input seam — confirmed.** `ServalLaneView::hit_test` plus xilem-serval's
   `dispatch_click` / `dispatch_key` ancestor walk (per the host plan doc). The
   leaf node is the hit target; forward to `Leaf::event`.
4. **Semantics seam — confirmed.** `a11y::accesskit_tree(dom, fragments, focus)`
   recurses the DOM via an internal `build`. The hook is: `build` consults the
   `LeafRegistry` for leaf nodes and lets `Leaf::accessibility` fill the node.

**Path-B registry — resolved (was open Q3).** netrender already owns the
registry: `Paint::install_external_texture(key: u64, texture: wgpu::Texture)`
registers, `DrawExternalTexture { texture_key }` references, and
`translate_envelope_with_external_textures` composites at frame time (WebGL
canvas e2e exercises the whole path). chisel needs **no** texture registry of its
own; a Path-B leaf derives a stable `u64` key from its `NodeId`, rasterizes its
scene to a `wgpu::Texture`, and installs it.

**Orrery port is a path upgrade, not just a branch move.** meerkat's orrery
currently composites through the host-side `compose_surfaces` /
`compose_external_texture` route with a hardcoded `ORRERY_SCENE_KEY`. As a chisel
Path-B leaf it moves onto the standard `install_external_texture` +
`DrawExternalTexture` paint-list path, so it gains in-paint-order z-placement and
sheds the bespoke host-composite branch.

## Resolved (2026-07-07)

- **Internal layout: arrangement leaves, not opaque.** A leaf with a private
  layout keeps children first-class via absolute placement, and z-order is native
  through serval's `paint_stacking`. See [Arrangement leaves](#arrangement-leaves).
- **Dirty granularity: two signals.** `paint_dirty` and `layout_dirty`, since
  they gate different passes.
- **Path-B texture registry: reuse netrender's** `install_external_texture`; no
  chisel-side registry. See [Seam verification](#seam-verification-2026-07-07).
- **First real consumer: meerkat's orrery surface**, retiring one hardcoded
  `*_SCENE_KEY` branch in `compose.rs`.

## Open questions

1. **`PaintCache` shape.** Path A caches a `Vec<PaintCmd>`; Path B caches a
   texture handle. One enum, or two leaf sub-traits? Leaning one `PaintCx` +
   one cache enum to keep the trait uniform.
2. **Prop application ergonomics.** The per-leaf view wrapper diffs typed data
   and pushes into the retained leaf. Is there a reusable helper, or is each
   leaf's wrapper bespoke like today's form controls?
3. **Arrangement-leaf reconciliation granularity.** First cut re-places all
   children on any child change; a dirty-tracked subset is the later refinement.
4. **crates.io name.** Confirm `chisel` is free; fall back within the craft
   cluster if taken.

## Relationship to existing docs

- [2026-05-27_serval_as_host_xilem_serval_plan.md](./2026-05-27_serval_as_host_xilem_serval_plan.md)
  — the reactive backend chisel leaves are authored through. chisel is the leaf
  layer that plan's views wrap.
- [2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md)
  — the paint seam. Path A emits its common `PaintCmd`s; Path B uses its
  `DrawExternalTexture` lowering. chisel adds no new `PaintCmd` variant.
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md)
  — the DOM seam a leaf attaches to as a replaced element.
