# C3 layout reshape plan — display-list emission to netrender Scene

**Status (archived 2026-05-17):** C3 landed. See the [c3 landed notes](../2026-05-08_c3_landed_notes.md) (still active) for the outcome. Kept here as the plan record.

---

Companion to [`2026-05-05_serval_netrender_cut_plan.md`](./2026-05-05_serval_netrender_cut_plan.md)
(see § C3). This doc captures the layout-side of C3 in the cut-plan
voice, scoped tightly enough for one focused multi-day session.

The C3 paint-side scaffold has landed: `components/paint/` is a
compile-clean `NetrenderPainter` stub (see § "Current state" below).
What remains is the layout-side reshape: `components/layout/` still
emits display lists via `webrender_api::DisplayListBuilder` calls
into a stub crate that doesn't have those types. The 124 layout
errors in `cargo check -p servo` are all this surface.

This plan is the rip-and-replace for that surface, ending at:
**`cargo check -p servo` passes through layout** (next blocker is
upstream, not layout's display-list emission).

---

## Current state (2026-05-06 baseline)

- ✅ `cargo check -p pelt` — clean (active path)
- ✅ `cargo check -p servo-paint` — clean (C3 paint scaffold)
- ✅ `cargo check -p servo-canvas-traits` / `servo-constellation` /
  `servo-paint-types` — clean
- ⏸ `cargo check -p servo` — **blocked at `servo-layout` with 124
  errors**, all unresolved imports / methods on `webrender_api::*`
  types that no longer exist (the empty `support/patches/webrender_api/`
  stub is intentional — see § C2 of cut plan).

The 124 errors are concentrated in **5 files** plus 2 trailing imports:

| File | Lines | Role |
|---|---|---|
| `components/layout/display_list/mod.rs` | 2200 | Display-list orchestration; `BuilderForBoxFragment` wrapper |
| `components/layout/display_list/stacking_context.rs` | 1933 | Stacking context tree, spatial nodes, clip chains |
| `components/layout/display_list/background.rs` | 369 | Background painting (color, image, gradient) |
| `components/layout/display_list/gradient.rs` | 468 | Linear / radial / conic gradient construction |
| `components/layout/display_list/conversions.rs` | 189 | `ToWebRender` trait + type conversions |
| `components/layout/layout_impl.rs` | (subset) | Display-list builder construction in layout phase |
| `components/layout/style_ext.rs` | 1 import | `wr::PrimitiveFlags` reference |

(The other display_list files — `clip.rs`, `hit_test.rs`,
`paint_timing_handler.rs` — were migrated to `paint_types` in earlier
slices and stay as-is.)

---

## Design choice — option (B) per cut plan

Per § C3 of the cut plan: "Servo's existing architecture is (B) —
display lists are sent across IPC to the paint thread. Keep that
shape."

Layout emits a `Vec<ServalDisplayItem>` (a serializable
intermediate). The painter (already scaffolded in
`components/paint/netrender_painter.rs`) translates to
`netrender::SceneOp` and pushes onto `netrender::Scene`.

**Why option (B) over (A) (layout owns Scene directly):**

- Preserves the script-thread / paint-thread IPC boundary that exists
  today; switching to (A) would require co-locating layout and the
  netrender Renderer, which fights the current process model.
- Keeps the `PaintMessage::SendDisplayList` shape stable in spirit
  (the payload becomes `Vec<ServalDisplayItem>` instead of
  `BuiltDisplayList`).
- Lets the painter own all netrender knowledge — the rest of layout
  doesn't import `netrender::*`.

---

## `ServalDisplayItem` shape

New enum, lives in `components/shared/paint/display_list.rs`
(replacing the existing 923-line file's webrender-shaped contents).
One variant per leaf operation layout pushes today.

```rust
// illustrative-signature-only — exact fields refined during impl
#[derive(Clone, Debug, Deserialize, Serialize, MallocSizeOf)]
pub enum ServalDisplayItem {
    // === primitives — map 1:1 to SceneOp variants ===
    Rect(RectItem),                          // → SceneOp::Rect
    Image(ImageItem),                        // → SceneOp::Image
    RepeatingImage(RepeatingImageItem),      // → SceneOp::Image (with tile metadata)
    Text(TextItem),                          // → SceneOp::GlyphRun
    Line(LineItem),                          // → SceneOp::Rect (1px-thick degenerate)
    Border(BorderItem),                      // → SceneOp::Stroke (per side)
    BoxShadow(BoxShadowItem),                // → SceneOp::Shape (with blur via filter)
    Shadow(ShadowItem),                      // → SceneOp::Shape (text shadow)
    Gradient(GradientItem),                  // → SceneOp::Gradient
    RadialGradient(RadialGradientItem),      // → SceneOp::Gradient (RadialKind)
    ConicGradient(ConicGradientItem),        // → SceneOp::Gradient (ConicKind)
    Iframe(IframeItem),                      // → SceneOp::Image (iframe surface key)

    // === structural — map to layer push/pop or nothing ===
    PushStackingContext(StackingContextDef), // → SceneOp::PushLayer
    PopStackingContext,                      // → SceneOp::PopLayer
    PushReferenceFrame(ReferenceFrameDef),   // → transform palette entry + active transform
    PopReferenceFrame,                       // → restore prior active transform

    // === clip / scroll — netrender currently has no direct ops ===
    DefineClipRect(ClipRectDef),             // → record in clip palette
    DefineClipRoundedRect(ClipRoundedRectDef),// → record in clip palette
    DefineScrollFrame(ScrollFrameDef),       // → record scroll node, no SceneOp
    DefineStickyFrame(StickyFrameDef),       // → record sticky node, no SceneOp

    // === hit-test surface — passes through painter to hit-test layer ===
    HitTest(HitTestItem),                    // → recorded; painter forwards to netrender::hit_test

    // === animation — out of scope for first cut ===
    RectWithAnimation(RectAnimItem),         // → SceneOp::Rect (animation deferred)
}
```

**What goes into the `*Item` structs** is the operation's payload
*after* unwinding webrender's `CommonItemProperties` (which bundled
clip, spatial, flags). In the new shape, each item carries its own
clip-id / spatial-id / flags as plain fields — no `CommonItemProperties`
indirection. This makes the IPC payload smaller and the painter's
translation more direct.

**What's deliberately not represented**:

- `BuiltDisplayList` — gone, the wire format is `Vec<ServalDisplayItem>`
  + `Vec<SpatialNode>` + `Vec<ClipDef>`.
- `DynamicProperties` (animated transforms / opacities) — first cut
  emits static values; animation hooks come in a follow-up.
- `RasterSpace` — netrender vello path is screen-space; no per-stack
  raster space control yet.

---

## DisplayListBuilder operation surface (for translation work)

Catalogued from `grep` over the 5 files + `layout_impl.rs`. Three
layers:

### 1. Layout-internal wrapper (`BuilderForBoxFragment` / display-list
builder context) — **stays as-is, just retargets**

~30 helper methods called by per-fragment painting code. These are
the right architectural layer for layout; they don't change shape.
What changes: the underlying field they wrap, from `wr::DisplayListBuilder`
to `Vec<ServalDisplayItem>` + a `ServalDisplayListContext` (clip /
spatial palette).

```text
add_all_spatial_nodes        check_if_paintable          padding_rect
add_clip_to_display_list     clip_chain_id                paint_body_background
begin / end                  common_properties            paint_dom_inspector_highlight
border_edge_clip             content_edge_clip            paint_info
border_radius                content_rect                 push_webrender_stacking_context_if_necessary
border_rect                  current_clip_id              reflow_statistics
build_background_image       current_reference_frame_scroll_node_id  spatial_id
check_for_lcp_candidate      current_scroll_node_id       wr (renamed → list)
                             device_pixel_ratio
                             dump_serialized_display_list
                             fragment
                             image_resolver
                             inspector_highlight
                             mark_is_contentful
                             mark_is_paintable
                             maybe_create_clip
                             padding_edge_clip
```

### 2. Layout's `push_*` helpers on the wrapper — **stays in shape, body changes**

Each wrapper method that emits a single primitive currently calls
`self.wr().push_*(...)`. New body: `self.list.push(ServalDisplayItem::*(...))`.

```text
push_rect                  push_box_shadow            push_radial_gradient
push_rect_with_animation   push_shadow                push_conic_gradient
push_image                 push_gradient              push_iframe
push_repeating_image       push_stops                 push_hit_test
push_text                  push_stacking_context      push_reference_frame
push_line                  push_border
```

### 3. Direct `wr::DisplayListBuilder` calls (`builder.wr().*`) — **rewrite call site**

Only handful of sites call into the unwrapped vendor builder
directly. These get replaced with the corresponding new helper or
direct `ServalDisplayItem` push.

```text
builder.wr().define_clip_rect          → list.push(DefineClipRect(...))
builder.wr().define_clip_rounded_rect  → list.push(DefineClipRoundedRect(...))
builder.wr().define_scroll_frame       → list.push(DefineScrollFrame(...))
builder.wr().define_sticky_frame       → list.push(DefineStickyFrame(...))
builder.wr().push_reference_frame      → list.push(PushReferenceFrame(...))
builder.wr().pop_reference_frame       → list.push(PopReferenceFrame)
builder.wr().push_border               → list.push(Border(...))
```

After this is done, the `wr()` method itself can be deleted from
`BuilderForBoxFragment`; nothing else calls it.

---

## Mapping table — `ServalDisplayItem` → `SceneOp`

(Painter-side — lives in `components/paint/netrender_painter.rs`'s
real implementation. Listed here to confirm every variant has a
target.)

| ServalDisplayItem | Painter action |
|---|---|
| `Rect` | `Scene::push_rect(SceneRect { ... })` |
| `Image` | `Scene::push_image(SceneImage { ... })` |
| `RepeatingImage` | `Scene::push_image` with tile-mode flag (FIXME: needs SceneImage extension or pre-tiled rect emission) |
| `Text` | `Scene::push_glyph_run(SceneGlyphRun { ... })` |
| `Line` | Degenerate `SceneRect` (1px thick along axis) |
| `Border` | Per-side `Scene::push_stroke(SceneStroke { ... })` (4 strokes for 4-sided border) |
| `BoxShadow` | `SceneShape` with blur applied via push_layer + filter, or pre-blurred raster (FIXME) |
| `Shadow` (text) | Same SceneShape + offset + blur |
| `Gradient` | `Scene::push_gradient(SceneGradient { kind: Linear, ... })` |
| `RadialGradient` | `SceneGradient { kind: Radial, ... }` |
| `ConicGradient` | `SceneGradient { kind: Conic, ... }` |
| `Iframe` | `Scene::push_image` with iframe surface as ImageKey, OR `declare_compositor_surface` if iframe gets its own native texture |
| `PushStackingContext` | `Scene::push_layer(blend, alpha, clip)` |
| `PopStackingContext` | `Scene::pop_layer()` |
| `PushReferenceFrame` | Append to `Scene::transforms`, set as active transform on subsequent ops |
| `PopReferenceFrame` | Restore prior active transform |
| `DefineClipRect` | Record in painter-side clip palette; subsequent ops carry clip-id |
| `DefineClipRoundedRect` | Same, with `BorderRadius` payload |
| `DefineScrollFrame` | Record in scroll-tree side data; no `SceneOp` (scrolling is netrender hit-test layer) |
| `DefineStickyFrame` | Same |
| `HitTest` | Forwarded to `netrender::hit_test::HitOp` registry; not a paint op |
| `RectWithAnimation` | First cut: `Scene::push_rect` ignoring animation; animation hook follow-up |

Items marked **FIXME** (RepeatingImage, BoxShadow, Shadow) need
either netrender extensions or pre-rasterization on the painter
side; first cut can panic-on-construct or stub these to emit a flat
fallback (solid-color rect, no shadow).

---

## File-by-file plan

### Step 1 — paint_api: define `ServalDisplayItem`

**Where**: `components/shared/paint/display_list.rs` (replace existing
923-line WebRender-shaped contents).

**What**:

- Define `ServalDisplayItem` enum with all variants above.
- Define the `*Item` payload structs.
- Define `ServalDisplayList` struct: `pub struct ServalDisplayList { pub items: Vec<ServalDisplayItem>, pub spatial_nodes: Vec<SpatialNodeDef>, pub clip_defs: Vec<ClipDef>, pub viewport_size: DeviceIntSize, pub pipeline_id: PipelineId }`.
- Helper methods: `ServalDisplayList::new(viewport, pipeline_id)`,
  `push(item)`, `define_spatial_node(...)`, `define_clip(...)` etc.
  These mirror `DisplayListBuilder::*` so the wrapper's methods
  retarget cleanly.
- Re-export from `components/shared/paint/lib.rs` alongside reshaped
  `PaintMessage` (see step 2).

**Done condition**: `cargo check -p servo-paint-api` succeeds; new
types are in scope.

**Estimated scope**: ~600 LOC of plain type definitions + helpers.

---

### Step 2 — paint_api: reshape `PaintMessage`

**Where**: `components/shared/paint/lib.rs` (existing 813 LOC).

**What**:

- The existing `PaintMessage` enum likely has `SendDisplayList`,
  `SendInitialTransaction`, `GenerateFrame`, `Animate*` variants
  per the cut plan's C2 description. Reshape:
  - `SendDisplayList(pipeline_id, ServalDisplayList)` — payload is
    the new shape.
  - `SendInitialTransaction(...)` — drop or rename to a
    netrender-shaped initial-state message.
  - `GenerateFrame` — keep, but its meaning becomes "ask the painter
    to materialize a Scene from the latest ServalDisplayList and
    invoke the Compositor."
- `WebRenderExternalImage*`, `WebRenderImageHandlerType`, etc. —
  rename without `WebRender` prefix (just `ExternalImage*`,
  `ImageHandlerType`).
- `WebRenderExternalImageIdManager` — keep name for now (used by
  servo.rs's `paint.webrender_external_image_id_manager()`); rename
  in a separate cosmetic cut.

**Done condition**: `cargo check -p servo-paint-api` and
`cargo check -p servo-paint` succeed against the reshaped surface.

**Estimated scope**: ~200 LOC of edits, mostly variant renames and
field-type swaps.

---

### Step 3 — layout: introduce `BuilderForBoxFragment` retarget

**Where**: `components/layout/display_list/mod.rs`.

**What**:

- Replace the field `webrender_display_list_builder: wr::DisplayListBuilder`
  with `serval_display_list: ServalDisplayList`.
- Rename method `wr()` → `list()` returning `&mut ServalDisplayList`.
- All `self.webrender_display_list_builder.*` calls in the file
  retarget to the new field. Most calls are `push_*` helpers that
  now route into `ServalDisplayList::push(...)`.
- Update imports: drop `use webrender_api::*` and `use webrender_api as wr`,
  add `use paint_api::display_list::*`.

**Done condition**: this file compiles.

**Estimated scope**: ~300 LOC of edits, mechanical mostly.

---

### Step 4 — layout: per-primitive `push_*` body rewrite

**Where**: `components/layout/display_list/mod.rs` (continued),
`components/layout/display_list/background.rs`,
`components/layout/display_list/gradient.rs`,
`components/layout/display_list/stacking_context.rs`.

**What**: Each of the ~16 `push_*` helpers currently calls
`self.wr().push_*(...)`. Rewrite to construct a `ServalDisplayItem::*`
and `self.list().push(...)`. Field-by-field translation per the
mapping table above.

The biggest payoff file is `mod.rs` (the orchestration); the others
follow the same pattern.

**Done condition**: `cargo check -p servo` reaches layout and
**passes**.

**Estimated scope**: ~600 LOC of edits.

**Risks**:

- `BoxShadow` / `Shadow` / `RepeatingImage` need design decisions
  during this step (panic-on-construct stub or fallback emission).
  Surface these explicitly when reached.
- `push_iframe` interaction with `declare_compositor_surface` — if
  iframes go to native compositor surfaces (per C4's StubCompositor
  design), this changes shape. First cut emits as `Image`; native-
  compositor iframes are a follow-up.

---

### Step 5 — layout: `conversions.rs` — `ToWebRender` rename + type maps

**Where**: `components/layout/display_list/conversions.rs`.

**What**:

- Trait rename `ToWebRender` → `ToServalDisplayItem` (or
  `ToPaintTypes` — match what the new type module exports).
- Type-conversion impls for the layout types that map into
  paint_types / display_list types — straightforward field
  substitutions.
- `box_fragment.rs` consumer calls `overflow.to_webrender()` —
  rename to `overflow.to_serval_display_item()` or keep the trait
  name `ToWebRender` for source-compat (cosmetic; favour the
  rename).

**Done condition**: `box_fragment.rs:445` compiles; `query.rs`
unaffected (uses `au_rect_to_length_rect`, not the trait).

**Estimated scope**: ~100 LOC of edits.

---

### Step 6 — layout: `stacking_context.rs` retarget + `layout_impl.rs` cleanup

**Where**: `components/layout/display_list/stacking_context.rs`
(1933 LOC), `components/layout/layout_impl.rs` (subset),
`components/layout/style_ext.rs` (1 import).

**What**:

- `stacking_context.rs`: imports drop, `wr::*` → `paint_api::*`,
  `define_*` calls retarget to `ServalDisplayList`, spatial-node
  construction works against `ServalDisplayList::spatial_nodes`
  vec. Big file but mechanical retarget per the inventory above.
- `layout_impl.rs`: replaces `webrender_api::DisplayListBuilder::new(pipeline_id)`
  with `ServalDisplayList::new(viewport, pipeline_id)`. Drops
  `use webrender_api::ExternalScrollId; use webrender_api::units::{...}`
  in favour of `use paint_types::*` (most types now exist there).
- `style_ext.rs`: `wr::PrimitiveFlags` reference. Either add a
  `PrimitiveFlags` stub to `paint_types::ids` (if any caller
  actually uses bit operations on it) or drop the field if
  unreachable. Inspect during step 6.

**Done condition**: `cargo check -p servo` passes through layout
end-to-end.

**Estimated scope**: ~400 LOC of edits.

---

### Step 7 — paint: real `NetrenderPainter` impl (handle_messages → Scene)

**Where**: `components/paint/netrender_painter.rs` (currently a stub).

**What**: Replace `handle_messages`'s no-op body with a real
translator.

- For each incoming `PaintMessage::SendDisplayList(pipeline_id, list)`:
  - Look up or create the `netrender::Scene` for that pipeline.
  - Walk `list.items`, dispatch per `ServalDisplayItem` variant per
    the mapping table.
  - Apply any clip-id / spatial-id state from the list's
    `clip_defs` / `spatial_nodes` palettes.
- For `PaintMessage::GenerateFrame(painter_id)`: call
  `Renderer::render_with_compositor(scene, format, &mut compositor, base)`.
- Hook the `netrender::Renderer` instance into the `Paint` struct
  (lives next to `paint_proxy` etc.).

**Done condition**: a single `<div>` with background color renders
end-to-end. (Doesn't require `cargo run`; a unit test that drives a
synthetic `ServalDisplayList` through the painter and checks the
resulting `Scene` is acceptable.)

**Estimated scope**: ~400-600 LOC. Largest single piece of new work
in C3.

---

## Order of operations

1. Step 1 (paint_api: ServalDisplayItem types) — independent.
2. Step 2 (paint_api: PaintMessage reshape) — depends on step 1.
3. Step 3 (layout: BuilderForBoxFragment retarget) — depends on step 1+2.
4. Step 4 (layout: per-primitive push bodies) — depends on step 3.
5. Step 5 (layout: conversions.rs) — depends on step 1.
6. Step 6 (layout: stacking_context.rs + layout_impl.rs + style_ext.rs) — depends on step 3.
7. Step 7 (paint: real NetrenderPainter) — depends on steps 1+2; can
   start in parallel with steps 3-6.

Steps 1-6 land at "`cargo check -p servo` reaches script through
layout cleanly." That's the layout-side done condition.

Step 7 lands at "synthetic display list renders to Scene" — proves
the painter side, doesn't yet require a running browser.

---

## What does *not* land in C3

- **C4 (`ServoCompositor` / `StubCompositor`)** — separate cut. C3
  ends at "`Renderer::render_with_compositor` is the call site
  shape," but the consumer-side `Compositor` impl is C4 work. Until
  C4 lands, the painter's `GenerateFrame` path can call
  `render_with_compositor` against a no-op stub Compositor.
- **WebGL display-list emission** — WebGL is dead per C1.5. No
  variant in `ServalDisplayItem` for it.
- **Animation properties** — first cut emits static values;
  `DynamicProperties` reshape is a follow-up.
- **Native compositor surfaces** (iframe → its own surface) — first
  cut treats iframes as `Image` items; `declare_compositor_surface`
  routing is C4 territory.

---

## Risks

1. **Hidden DisplayListBuilder methods** not in the catalogue. The
   surface above came from a single-pass grep; if step 3 surfaces
   a method I missed, add it to `ServalDisplayList` and continue.
2. **`SpatialId` / `ClipChainId` semantics** — these are palette
   indices in the wire format. Layout uses them as opaque tokens.
   New shape preserves opaqueness, but the construction/lookup site
   in stacking_context.rs is non-trivial. Step 6 is where this
   lands.
3. **Reference-frame transform stack** — webrender tracks the
   active transform implicitly via push/pop; the new shape must
   too. `ServalDisplayList::transforms_palette + active_transform_id`
   is the proposed shape. Confirm at step 3.
4. **Painter's translation completeness** (step 7) — if a variant
   has no clean SceneOp, the painter panics or stubs. List the
   sticky cases (BoxShadow, RepeatingImage, etc.) up-front so
   they're not a surprise.

---

## Post-C3: what unblocks

- `cargo check -p servo` passes end-to-end (modulo any other
  upstream issue surfaced after layout passes).
- C5 (script dep removal from layout) becomes mechanical — without
  the webrender_api tangle, decoupling layout from `script` is
  trait-shuffling only.
- C4 (ServoCompositor) becomes the next render-side cut.
- The fork's renderer is netrender-driven through the full path:
  layout → ServalDisplayItem → SceneOp → Renderer → Compositor.

---

## When this plan is done

When `cargo check -p servo` passes through layout (step 6 done) and
the synthetic painter test passes (step 7 done), C3 lands.

Move this plan to `docs/archive/` at that point; the imposed shape
is the codebase.
