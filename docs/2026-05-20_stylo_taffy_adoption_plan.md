# stylo_taffy adoption + Stylo 0.16 isolation + floats

Status: **DONE (2026-05-25)** — the hand-written property mapping is
gone; `cv_to_taffy.rs` now delegates every property to
`stylo_taffy::convert::*`. Floats land (block-level), verified by an
e2e pixel test. The one done-condition originally **reframed** (delete
`cv_to_taffy.rs`) was later **fully met** when the box-tree
re-architecture landed and retired the file — see
[Outcome](#outcome-2026-05-25). Originally planned 2026-05-20.

## Decision

1. **Adopt `stylo_taffy`** (crates.io `0.3.0-alpha.2`) as serval-layout's
   `ComputedValues → taffy::Style` mapping, replacing the hand-written
   `cv_to_taffy.rs`.
2. **Isolate serval-layout on Stylo 0.16.0** (crates.io). Servo's
   `script` / `layout` (and the rest of the Servo-derived stack) stay on
   the `servo/stylo` git pin at `0.12.0`. Two Stylo copies coexist in the
   build until the old layout path is cut.
3. **Floats** ride in with the adoption: enable `stylo_taffy/floats`
   (`= ["taffy/float_layout"]`) and `taffy/float_layout`. Not hand-written
   on 0.12 first — the mapping comes from the maintained crate.

## Why this shape

- **Same crate, different version.** serval pins `servo/stylo`
  git rev `572ecba` which declares version **0.12.0**; stylo_taffy/blitz
  want **0.16.0**. Same lineage (crates.io `stylo` is published from
  `servo/stylo`), but four minor versions apart — the
  `ComputedValues` / `TElement` surface moved, so it's a port, not a
  `[patch]`.
- **serval-layout is a clean isolation boundary.** servo-paint is
  Stylo-free (it consumes only `ServalPaintList` — verified). The e2e
  path (serval-layout → ServalPaintList → paint → netrender) crosses
  Stylo types *only inside serval-layout*. So serval-layout can move to
  0.16 without dragging Servo's script/layout along.
- **taffy version already matches.** stylo_taffy `0.3.0-alpha.2` wants
  `taffy 0.10.1` — exactly serval's pin. Zero taffy skew.
- **Floats algorithm is upstream.** `taffy/float_layout` ships in the
  0.10.1 serval already has; stylo_taffy maps `float`/`clear` into it.

## Known limit (floats)

serval's inline formatting context is a **parley-measured opaque leaf**
to Taffy. Taffy's float layout can shift sibling *blocks* around a
float, but cannot reach into a parley leaf to shorten line boxes — so
*text-wrapping-around-a-float* will not work without integration at that
seam (the same seam that makes the IFC a "measured leaf" rather than a
true IFC). Block-level float interaction is the realistic target;
text reflow around floats is a later, deeper piece — see blitz-dom as
the reference for how to reconcile parley line boxes with taffy floats.

## Resolved facts (2026-05-20, during execution)

The investigation overturned two assumptions:

- **serval is already on Stylo 0.17.0** (git rev `572ecba` = the
  published 0.17.0; the `0.12.0` I first read was a *different* stale
  checkout's workspace version). So there is **no Stylo API port** — the
  whole workspace already runs current Stylo. The earlier
  "isolate serval-layout on 0.16" framing is moot.
- **`stylo` declares `links = "servo_style_crate"`**, so cargo permits
  exactly one Stylo in the graph — isolation was never possible. It's
  one Stylo, workspace-wide.

Therefore the adoption is: keep the single git Stylo 0.17, add a
`[patch.crates-io]` so `stylo_taffy`'s crates.io `stylo`/`stylo_atoms`
deps resolve to serval's git rev (unifying past `links`), and take the
Taffy version `stylo_taffy` pins.

That Taffy version is **`=0.11.0-experimental-cache-fix.3`** — a
blitz-aligned pre-release (the only 0.11.x). Chosen deliberately
(2026-05-20) over hand-porting on stable 0.10.1: accept the pre-release
pin to get the maintained mapping + blitz alignment. Reach of the bump:
**serval-layout** + **paint's dev-dependency** (its tests call
`serval-layout::layout` with `taffy::Size` args, so they must match).
Servo's `components/layout` and `malloc_size_of` stay on Taffy 0.10.1 —
no shared taffy-typed boundary with serval-layout, so the two versions
coexist (taffy has no `links`).

`stylo_taffy`'s `floats` feature = `["taffy/float_layout"]`, so floats
come from the crate's mapping, not hand-written.

## Migration steps

1. **Deps.** In `components/serval-layout/Cargo.toml`, switch the
   stylo-family deps from `workspace` (git 0.12) to explicit crates.io
   `0.16.0`: `stylo` (pkg, imported as `style`), `selectors`,
   `servo_arc`, `stylo_atoms`, `stylo_dom`, `stylo_traits`. Add
   `stylo_taffy = { version = "0.3.0-alpha.2", features = ["floats"] }`.
   Add `float_layout` to the `taffy` features.
2. **Port the Stylo-internal files** to the 0.16 API (bounded set):
   - `adapter_stylo.rs` — the `TElement`/`TNode`/`TDocument` impls.
   - `cascade.rs` — stylist construction + `cascade` invocation.
   - `style.rs` — `ElementData` / `ElementDataWrapper` shape.
   - `font_metrics.rs` — any `FontMetricsProvider` surface drift.
   The reads in `construct.rs` / `paint_emit.rs` / `image_decode.rs`
   (font-size, color, background-image, box-shadow) track minor renames
   only.
3. **Swap the converter.** Delete `cv_to_taffy.rs`; route
   `StylePlane::refresh_taffy_from_cascade` through
   `stylo_taffy::to_taffy_style` (or `TaffyStyloStyle` zero-copy wrapper).
4. **Floats.** Confirm `taffy/float_layout` actually drives block
   displacement at 0.10.1 (not a stub); add a block-level float e2e test
   (`float: left` shrinks/sits beside a sibling block). Document the
   text-wrap limit in the test.
5. **Verify.** serval-layout lib tests + the html_to_pixels_e2e suite
   green. Watch for a second Stylo copy bloating build — acceptable for
   now, noted as the reconciliation debt.

## Blitz survey — adopt crate vs adopt idea

| Crate | Verdict | Rationale |
|---|---|---|
| `stylo_taffy` | **adopt crate** | Pure `ComputedValues→Style`; no architectural opinion; exact taffy match. |
| `stylo_to_kurbo` (in blitz-dom) | **adopt crate, later** | `resolve_2d_transform`: CSS transform → kurbo matrix, for the existing `transform_id` path. Small, clean. |
| `blitz-traits` | **adopt ideas** | Host seams only (events/net/navigation/shell). serval's host story is the Hekate lanes / Mere ecosystem; borrow shapes, not the crate. |
| `blitz-dom` | **adopt ideas, not the crate** | Reference for the hard problems (parley-line-boxes-in-taffy-with-floats, replaced/inline boxes, restyle/invalidation). But it is a **Node-tree** DOM; serval's **planes** model (StylePlane/FragmentPlane/ImagePlane keyed by NodeId + query traits) is a different, deliberate design. Steal techniques; keep planes. |
| `blitz-html` | **adopt ideas / skip** | markup5ever HTML parsing; serval has `serval_static_dom`. |

Principle (the user's framing): adopt the crate when it *agrees* with the
planes architecture (stylo_taffy is a pure function — yes); adopt only
the *ideas* when the crate embeds a conflicting model (blitz-dom's tree).

## Done conditions

- serval-layout compiles against crates.io Stylo 0.16 + stylo_taffy;
  Servo script/layout untouched on git 0.12.
- `cv_to_taffy.rs` deleted; layout driven by `stylo_taffy::to_taffy_style`.
- All serval-layout lib tests + html_to_pixels_e2e green.
- A block-level float renders correctly in an e2e pixel test; the
  text-wrap-around-float limit is documented, not silently broken.

## Outcome (2026-05-25)

Adopted and verified. Status of each done-condition against the tree:

- ✅ **serval-layout compiles against stylo_taffy.** `Cargo.toml`:
  `stylo_taffy = { version = "0.3.0-alpha.4", features = ["floats"] }`,
  `taffy = "=0.11.0-experimental-cache-fix.3"` with `float_layout`
  enabled. (The "Stylo 0.16 isolation" framing was already overtaken by
  the Resolved-facts section — one git Stylo, workspace-wide. No port.)
- ✅ **The hand-written property mapping is gone.** `cv_to_taffy.rs`'s
  `to_taffy_style` delegates *every* property — display, box-sizing,
  position/inset, overflow, float/clear, sizing (incl. min/max +
  aspect-ratio), margin/padding/border, gap, flexbox — to
  `stylo_taffy::convert::*`. This is the substance the plan set out to
  achieve: serval no longer carries its own `ComputedValues → Style`
  logic.
- ✅ **`cv_to_taffy.rs` deleted (2026-05-25, via the box tree).** This
  bullet was first written as ⚠️ "reframed, not deletable" — the
  explanation below stands as the *why it took a re-architecture*, but
  the file is now gone. The literal done-condition ("delete
  `cv_to_taffy.rs`; drive via the zero-copy `TaffyStyloStyle`") was
  **not achievable under the owned-`Style` `TaffyTree`**, for a real
  taffy-API reason, not laziness:
  - `stylo_taffy::to_taffy_style` returns `taffy::Style<Atom>` (the
    `Atom` carries CSS-grid *template line-names*).
  - serval stores nodes in `TaffyTree<InlineContent<NodeId>>`, and
    `TaffyTree<NodeContext = ()>` is **not generic over the ident
    type** — its stored `style` field and `new_leaf`/`set_style` all
    take `Style` (= `Style<DefaultCheapStr>`, the hardcoded default).
    So `Style<Atom>` cannot be stored without a per-field rebuild —
    which is precisely what `cv_to_taffy::to_taffy_style` *is*. Calling
    `to_taffy_style` then field-copying `Style<Atom> → Style<…>` would
    be *more* code than the current direct assembly, and still
    hand-written.
  - The only way to *literally* retire the file is to abandon the
    owned-`Style` `TaffyTree` for taffy's **trait-impl tree** — make
    serval's planes implement `LayoutPartialTree`/`*ContainerStyle` and
    feed `TaffyStyloStyle` (zero-copy over `ComputedValues`) directly.
    That's how blitz-dom does it. It's a tree re-architecture, not a
    converter swap — so it was split into its own plan
    ([2026-05-25_box_tree_trait_impl_plan.md](./2026-05-25_box_tree_trait_impl_plan.md)),
    which **landed 2026-05-25**: the box tree feeds `TaffyStyloStyle`
    zero-copy, `cv_to_taffy.rs` is deleted, and `StyleEntry` no longer
    carries an owned `taffy::Style`.
- ✅ **Block-level float e2e pixel test green.**
  `components/paint/tests/html_to_pixels_e2e.rs::html_to_pixels_float_left_places_blocks_side_by_side`
  — two `float: left` divs sit side-by-side (where plain blocks stack),
  verified at the pixel level on real GPU. The text-wrap-around-float
  limit is documented in the test body, not silently broken.
- ✅ **Tests green.** `serval-layout --lib`: 34 passed. The float e2e
  test passes through the full HTML → cascade → layout → emit → render
  → readback path.

Net: the adoption is **done** — no hand-rolled mapping, floats land,
tests green. The one literal miss at first writing (file deletion) was
an artifact of taffy's non-generic `TaffyTree`; rather than silently
drop the condition, it was recorded with the trait-impl-tree path named
as the only way to close it — and that path was then taken (the box
tree), so the file is deleted and the condition fully met.
