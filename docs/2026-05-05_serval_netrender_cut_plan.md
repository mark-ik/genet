# serval — netrender cut plan (C1.5 — C4)

Companion to the C1 commit (`651a83b62cd`, *cut GL/surfman corpus
from rendering-context layer*). Captures the imposed shape this fork
is moving toward, the four cuts left after C1, and the contract
each cut ends at.

Pattern (per the netrender bring-up that succeeded): **rip the
parallel codepath, fix what breaks, don't try to incrementally
migrate**. Each cut is "delete the corpus, run cargo check,
resolve the holes."

---

## The imposed shape

```
serval display-list lowering
        ↓ emits
    netrender::Scene (SceneOp painter order)
        ↓ feeds
    netrender::Renderer::render_with_compositor(scene, format, &mut compositor, base)
        ↓ hands master texture + LayerPresent slice to
    serval::ServoCompositor (impl netrender_device::Compositor)
        ↓ blits dirty surface regions into
    serval-owned native textures (IOSurface / DXGI / Wayland subsurface)
        ↓ presented via
    OS compositor
```

No webrender. No DisplayListBuilder. No surfman. No GL stack. The
display-list lowering reshapes to emit `SceneOp` painter-order ops
directly. The old `BuiltDisplayList` shape is replaced by netrender's
`Vec<SceneOp>`.

---

## Ordering rationale

C1.5 and C2 are independent. C2 unblocks compile-clean (currently
the 81 errors in `components/malloc_size_of/lib.rs` are all
webrender_api stub gaps). C1.5 is opportunistic dead-code removal.

C3 depends on C2 (replaces the deleted `components/paint/`). C4
depends on C3 (the painter is what calls the Compositor). C1.5 can
slot in at any natural break.

**Recommended order:** C2 → C3 → C4 → (C1.5 anywhere).

---

## C1.5 — WebGL corpus removal

**Why:** WebGL is the next-largest GL consumer. After C1 removed
the surfman-backed RenderingContext impls, GL workspace deps
(`gleam`, `glow`, `surfman`, `mozangle`, `swgl`, `glsl`) survive
only because WebGL still uses them.

**Trigger:** When fork-direction commits to "no WebGL." A
graphshell-shaped consumer doesn't need it; smolweb doesn't render
3D canvas. WebXR fate is coupled — see deferred decisions below.

**Cuts:**

- `components/canvas/webgl_*` — delete
- `components/script/dom/webgl/` — delete (40+ files)
- `components/webgl/` — delete
- `components/shared/canvas/` — trim WebGL-only bits, keep 2D canvas surface area
- `gleam`, `glow`, `surfman`, `mozangle`, `swgl`, `glsl-to-cxx` from workspace `[workspace.dependencies]`

**Knock-on:** `components/script/dom/canvas/2d/canvas_state.rs` and
`components/script/dom/canvas/canvas_context.rs` reference WebGL
variants of their `OffscreenRenderingContext` enum (the **canvas
API** enum, not the deleted shared/paint enum) — those variants
get pruned.

**Done condition:** workspace `cargo check` doesn't pull `gleam`,
`glow`, `surfman`, `mozangle`, `swgl`, or `glsl-to-cxx`. The
`<canvas>` element still works for 2D contexts; `getContext('webgl')`
returns null or panics with a clear error.

**Deferred decisions:**

- **WebXR**: also GL-coupled today (via webxr crate). Either delete
  with WebGL or keep as a feature-gated stub. Pick at C1.5 start.
- **WebGPU**: independent of GL stack — *stays*. No conflict with
  C1.5.

**Scope:** dozens of files deleted; net code reduction. ~1-2 days
of focused work.

---

## C2 — Cut webrender_api / wr_malloc_size_of corpus

**Why:** netrender's display model is `Scene` + `SceneOp`, not
display lists. Every `webrender_api::*` import on the consumer side
is a coupling that doesn't translate. The empty stubs from the
post-rebase fixup commits aren't a long-term answer (81 errors
prove the stubs can't satisfy real consumers).

**The shape change:**

webrender_api's role in current servo-wgpu is two-fold:
1. **Type definitions** consumed by script/dom/layout for
   display-list construction (`ImageKey`, `ColorF`, `BorderRadius`,
   `units::DeviceIntSize`, `MixBlendMode`, `FontKey`,
   `ExternalScrollId`, etc. — 36 distinct symbols).
2. **Display-list builder + frame submission** consumed by
   `components/paint/` (`DisplayListBuilder`, `RenderApi`,
   `Transaction`, `Document`).

(2) goes away wholesale (`components/paint/` deleted, replaced by
C3). (1) needs a destination — the types still need to exist
because layout / script / canvas all reference them. That
destination is a **new servo-owned crate**: `serval-paint-types`
(or `paint_types`), with three concerns:

- ID types (`ImageKey`, `FontKey`, `FontInstanceKey`, `PipelineId`,
  `Epoch`, `ExternalScrollId`, `SpatialId`, `DocumentId`,
  `ExternalImageId`) — plain newtype wrappers, no webrender
  dependency.
- Color/geometry/style types (`ColorF`, `BorderRadius`,
  `BorderStyle`, `LineStyle`, `MixBlendMode`, `TransformStyle`,
  `RepeatMode`, `ImageRendering`, `ImageDescriptor*`,
  `NormalBorder`, `StickyOffsetBounds`, `ReferenceFrameKind`,
  `PropertyBindingKey`) — plain Rust types, optionally derived
  Serialize/MallocSizeOf.
- Units re-export — `serval-paint-types::units` mirrors
  `webrender_api::units`'s shape, backed by `euclid` directly.

**Cuts:**

- `components/paint/` — delete entirely (webrender wrapper).
- `components/shared/paint/lib.rs` — major reshape. The `PaintMessage`
  enum drops every WebRender-specific variant (`SendInitialTransaction`,
  `SendDisplayList`, `GenerateFrame` becomes Scene-shaped).
  `WebRenderExternalImage*`, `WebRenderImageHandlerType` —
  rename without "WebRender" prefix or replace with netrender's
  image registry concept.
- `support/patches/webrender/`, `support/patches/webrender_api/`,
  `support/patches/wr_malloc_size_of/` — delete the stubs.
- Workspace `webrender`, `webrender_api`, `wr_malloc_size_of` from
  `[workspace.dependencies]` and `[patch.crates-io]` — delete.
- The `malloc_size_of_is_webrender_malloc_size_of!` macro in
  `components/malloc_size_of/lib.rs:1212-1223` + every invocation
  (~50 of them in nearby lines) — delete. New types in
  `serval-paint-types` derive `MallocSizeOf` directly.
- Every `use webrender_api::*` import in script / layout / canvas /
  webgpu — replace with `use paint_types::*` (mostly mechanical
  sed) OR delete the call site if it's webrender-specific
  (DisplayListBuilder calls, Transaction sends).

**New scaffolding:**

`components/shared/paint-types/` (illustrative-signature-only):

```rust
// paint_types/src/lib.rs
pub mod units;       // euclid-backed mirrors of webrender_api::units
pub mod color;       // ColorF
pub mod border;      // BorderRadius, BorderStyle, NormalBorder, LineStyle
pub mod composite;   // MixBlendMode, TransformStyle, ImageRendering
pub mod gradient;    // RepeatMode, ReferenceFrameKind
pub mod ids;         // ImageKey, FontKey, FontInstanceKey, PipelineId,
                     // Epoch, ExternalScrollId, SpatialId, DocumentId,
                     // ExternalImageId
pub mod image;       // ImageDescriptor, ImageDescriptorFlags,
                     // ImageFormat, SerializableImageData (move from
                     // shared/paint/lib.rs)
pub mod sticky;      // StickyOffsetBounds
pub mod property;    // PropertyBindingKey
```

Each type derives `Serialize`, `Deserialize`, `Clone`, `Debug`, +
`MallocSizeOf` where needed. No webrender dep, no
wr_malloc_size_of dep. Just servo's own `malloc_size_of` crate.

**Done condition:** `cargo check -p servo-paint-api` succeeds.
`grep -r "webrender_api\b" components/` returns zero results.
Workspace deps no longer reference webrender / webrender_api /
wr_malloc_size_of.

**Scope:** ~100 files touched. Most are import-renames (mechanical
sed). The new `paint_types` crate is a few hundred lines of
straightforward type definitions. ~2-3 days of focused work; can
be sliced (script first, then layout, then canvas, then webgpu) so
each slice ends at "compiles."

**Deferred decisions:**

- **`paint_types` vs `serval-paint-types` crate name** — cosmetic;
  pick at scaffold time. Lowercase, snake-case is servo style.
- **Move `SerializableImageData` etc. from `shared/paint/lib.rs`
  to `paint_types`** — yes for cleaner factoring, but means more
  movement at C2 time. Punt to C3 if scope feels large.
- **`webxr` and `webgpu` reference webrender_api types too** —
  webxr likely dies in C1.5; webgpu needs to migrate (its
  `ExternalImageId`, `ImageDescriptor` references map cleanly to
  paint_types).

---

## C3 — Build new netrender-driven painter

**Why:** C2 deleted `components/paint/` (the webrender wrapper).
Something has to do paint's job: receive display lists from script
threads, lower them to `netrender::Scene`, drive
`netrender::Renderer`. C3 is that something.

**The shape:**

New `components/paint/` (same crate name and location, new body):

```rust
// painter.rs (illustrative-signature-only)
pub struct NetrenderPainter {
    renderer: netrender::Renderer,
    scenes: HashMap<PainterId, netrender::Scene>,
    surfaces: Vec<netrender::CompositorSurface>,
    compositor: Box<dyn netrender_device::Compositor>,
    image_keys: ImageKeyAllocator,
    font_keys: FontKeyAllocator,
    // ...
}

impl NetrenderPainter {
    pub fn new(handles: WgpuHandles, compositor: Box<dyn Compositor>) -> Self;
    pub fn handle(&mut self, msg: PaintMessage);
    pub fn render_frame(&mut self);  // calls Renderer::render_with_compositor
}
```

**Cuts:**

- The old `components/paint/` body was deleted in C2; this is
  net new code, not a delete-and-replace at the Rust level.
- The `PaintMessage` enum in `shared/paint/lib.rs` was reshaped
  in C2 — C3 implements the handler for the new shape.

**Display-list lowering — where it goes:**

Today, layout produces a `BuiltDisplayList` (webrender's display
list shape) via `DisplayListBuilder`. Post-C2, layout emits a
sequence of operations against a netrender-shaped builder
instead. Two design choices for that builder:

- **(A)** Layout owns a `netrender::Scene` directly and pushes
  `SceneOp`s. Tightest coupling but matches the shape exactly.
- **(B)** Layout emits a serializable intermediate
  (`Vec<DisplayItem>` or similar), and the `NetrenderPainter`
  translates to `SceneOp`. Decouples layout from netrender's
  exact API; lets the intermediate be shipped IPC-cheap to a
  paint thread.

Servo's existing architecture is (B) — display lists are sent
across IPC to the paint thread. Keep that shape. The intermediate
becomes a `Vec<ServalDisplayItem>` or similar; the painter
translates.

**Done condition:** A minimal smoke test: a single `<div>` with
background color renders end-to-end via the new painter. The
painter receives a display list, lowers it, calls
`Renderer::render_with_compositor`, the stub Compositor (or a real
one, if C4 is done) presents.

**Scope:** ~1000-2000 lines of new code (painter logic +
display-list-to-Scene translator). Multi-day focused work.

**Deferred decisions:**

- **Multi-painter support** — servo runs one painter per WebView.
  Match that or simplify to one painter for the cut. Pick at
  scaffold time.
- **Tile cache invalidation strategy** — netrender's `TileCache`
  exists; whether layout drives invalidation hints or netrender
  detects via hashing alone is a profile-driven choice. Default:
  let netrender handle it.
- **Font handling** — netrender uses `FontBlob` + `Glyph`;
  servo's text path uses `FontKey` + `GlyphInstance`. Translation
  layer needed; lives in the painter.

---

## C4 — Build ServoCompositor adapter

**Why:** `netrender::Renderer::render_with_compositor` requires a
`Compositor` impl. C4 is that impl, plus the OS-handoff platform
glue.

**The shape:**

```rust
// servo_compositor.rs (illustrative-signature-only)
pub struct ServoCompositor {
    handles: WgpuHandles,
    // Per-surface destination texture pool
    destinations: HashMap<SurfaceKey, ServalSurface>,
    // OS-side compositor backend
    os_compositor: Box<dyn OsCompositorBackend>,
}

impl netrender_device::Compositor for ServoCompositor {
    fn declare_surface(&mut self, key: SurfaceKey, world_bounds: [f32; 4]) {
        // Allocate / reallocate native texture for this surface
    }
    fn destroy_surface(&mut self, key: SurfaceKey) {
        self.destinations.remove(&key);
        self.os_compositor.destroy(key);
    }
    fn present_frame(&mut self, frame: PresentedFrame<'_>) {
        // Encode copy_texture_to_texture for each dirty layer
        // Submit copies via frame.handles.queue
        // Hand native textures to OS compositor with transform/clip/opacity
    }
}

trait OsCompositorBackend {
    fn declare(&mut self, key: SurfaceKey, native: &wgpu::Texture);
    fn present(&mut self, key: SurfaceKey, transform: Affine, clip: Option<Rect>, opacity: f32);
    fn destroy(&mut self, key: SurfaceKey);
}

// Platform impls
struct WindowsDxgiCompositor { /* DXGI Composition */ }
struct MacosCalayerCompositor { /* CALayer */ }
struct WaylandSubsurfaceCompositor { /* wayland-subsurface */ }
struct StubCompositor { /* fullscreen single-surface fallback */ }
```

**Cuts:** none — C4 is net new code in `components/paint/` (or a
new sibling crate `components/compositor/` for clarity).

**Done condition:** A `<div>` renders into a serval-owned
native texture; on macOS, a CALayer presents that texture; on
Windows, a DXGI Composition Visual; on Linux, a Wayland
subsurface. For the cut milestone, the **StubCompositor** (single
fullscreen surface, blits master directly to the swapchain
texture) is enough — platform-specific impls are post-cut work.

**Scope:**

- Trait + ServoCompositor + StubCompositor: ~300 lines.
- Per-platform OS handoff: ~300-500 lines each. Windows DXGI is
  most documented; macOS CALayer well-understood; Wayland
  subsurfaces require live testing.

**Deferred decisions:**

- **Surface allocation policy** — does netrender's
  `Scene::declare_compositor_surface` get called per scrolling
  region? Per iframe? Per CSS `will-change: transform`? The
  granularity is policy-driven; default to "iframe + fixed
  positioning + will-change" matching webrender's slicing
  heuristic.
- **Sub-tile damage** — netrender's path-(b′) gives surface-
  granularity damage; finer-grained damage requires either
  netrender extensions or consumer-side overpaint detection.
  Defer to profile-driven.

---

## What stays untouched across C1.5–C4

- `components/script/` — content side. Tons of `webrender_api`
  imports today; these are C2 sed targets but the script logic
  itself doesn't change.
- `components/layout/` — display-list emission shape changes
  (C2), but layout algorithms unchanged.
- `components/canvas/` 2D path — kept (only the WebGL canvas
  variants die in C1.5).
- `components/webgpu/` — independent of C1.5/C2; might reference
  webrender_api types for ExternalImageId etc. (C2 migrates
  those).
- `components/constellation/`, `components/script_bindings/`,
  `components/net/`, `components/storage/`, etc. — pure servo
  components untouched by the cut.
- `RenderingContextCore` + `WgpuRenderingContext` (the trait
  split work that C1 preserved) — these are the foundation C3/C4
  build on top of.

---

## What gets renamed

Independent of C1.5–C4, the workspace itself has a pending rename:
**`servo-wgpu/` → `serval/`**. Cosmetic, one-time, ~5 minutes:

1. Rename directory.
2. Update path references in sibling workspaces (none today
   except `wgpu-graft/` which references `../servo-wgpu/...`).
3. No code changes.

Schedule: at any natural break — recommend after C2 lands (when
the workspace is buildable again) or after C3 (when the imposed
shape is concretely visible).

---

## Risks not already covered

1. **Servo upstream pulls during C2/C3** will conflict heavily
   with the cut. Plan: rebase upstream-mirror periodically (small
   updates), but don't pull into `main` mid-cut. Big upstream
   surges go into `servo-mirror`, get merged into `main` only at
   end-of-phase.

2. **Servo's display list is over a decade of CSS-spec
   accumulation.** Netrender's `Scene` model is younger. C3 may
   surface display-list features that don't have netrender
   equivalents (CSS filters chained with mix-blend-mode under
   3D transforms, etc.). Strategy: ship the smoke test first,
   add features as real test pages surface them.

3. **The `Compositor` API surface in netrender** was designed for
   graphshell-shaped consumers. Servo's needs (multiple webviews,
   nested iframes, scrolling regions) may force netrender API
   extensions. Path: track those as netrender roadmap items, not
   serval blockers — the path-(b′) Compositor design is
   intentionally extensible.

4. **wgpu version drift** — netrender pins wgpu 29; servo may
   want different. Workspace pins wgpu 29 today; future updates
   require coordinated bumps.

5. **Forking divergence** — once C2-C4 land, this fork can no
   longer cleanly merge upstream servo's renderer changes (they
   touch webrender; we have no webrender). Acceptable: this is
   the cost of going off-trail. Upstream-mirror branch preserves
   the option of cherry-picking specific commits selectively.

---

## When this plan becomes stale

Move an entry into a `§Cx — CLEARED` section here when it lands.
Add new entries when post-cut surprises appear. Delete the whole
plan when serval's renderer is netrender-driven and webrender is
gone — at that point the imposed shape *is* the codebase.
