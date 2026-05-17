# netrender — native-compositor handoff via path (b′) (2026-05-05, revised same day)

**Status (archived 2026-05-17):** Netrender-side sub-phases 5.1, 5.2, 5.3, 5.4 shipped (per the doc's own 2026-05-05 implementation-status note, with receipts in netrender's `p13prime_path_b_present_plumbing.rs`). Serval-side sub-phase 5.5 (the consumer adapter) has landed via the DXGI compositor and CALayer compositor modules in `components/paint/compositor_dxgi.rs` and `components/paint/compositor_calayer/` (see also commit `8f88d8de343` "Update DXGI compositor and add platform handoff documentation" and `10420ed6258` "split netrender_painter + compositor_calayer into module dirs"). Kept here as the design rationale + sub-phase ordering for the work that landed.

---

Design plan for axiom-14 (rasterizer plan §1, §13) — exporting
per-surface textures to native OS compositors (CALayer / IOSurface
/ DXGI Composition / Wayland subsurfaces) so the OS can apply
transform / clip / opacity at 60Hz without re-rasterization.

Distinct from:

- [`2026-05-01_vello_rasterizer_plan.md`](2026-05-01_vello_rasterizer_plan.md)
  §2.4 — the cost-shape table that originally documented "trivial
  handoff lost" as an accepted trade for Masonry. **This plan
  recovers most of that loss without un-doing Masonry.**
- The doc's earlier [§13' deferral discussion](archive/2026-05-05_deferred_phases.md)
  — superseded by this plan.

Consumer: **serval** (in workspace), reshaping its compositing
layer to consume the `Compositor` trait defined here.

**Revision note (2026-05-05).** Initial draft had a crate-cycle
bug (importing `netrender::scene::Rect` into a `netrender_device`
type) and an inconsistent copy-ownership model (trait gave
consumer no destination textures, but render-path swap claimed
netrender did the copies). Both are fixed below; copy ownership
is now firmly **consumer-side via `present_frame`**.

**Implementation status (2026-05-05).** Netrender-side sub-phases
**5.1, 5.2, 5.3, 5.4 shipped** with eight receipts in
[`p13prime_path_b_present_plumbing.rs`](../netrender/tests/p13prime_path_b_present_plumbing.rs).
Sub-phase **5.5** (serval adapter) is the remaining load-
bearing piece and lives in the `serval` repo, separate
workspace.

---

## 1. Why path (b′), not (a) or (b)

| Path | Render submits/frame | Damage info | OS sees real native textures | Verdict |
| --- | --- | --- | --- | --- |
| (a) per-tile vello render | N (one per tile) | per-tile | yes | expensive at scale, vello encoder API may not allow batching |
| (b) one render + slice | 1 render + 1 unconditional copy | **lost at API level** | yes | doc dismissed for damage loss |
| **(b′) one render + slice + damage exposed** | 1 render + ≤1 copy (only when dirty) | per-declared-surface | yes | **chosen** |

(b)'s dismissal in the original deferred-phases doc was wrong:
the damage info isn't lost, it's just not exported. `TileCache`
already returns dirty `TileCoord` lists from `invalidate(scene)`.
Path (b′) computes per-declared-surface dirty bits from those and
exposes them through the `Compositor::present_frame` API. Clean
surfaces skip their copy.

**Submit count, precise.** netrender always pays one
`vello::Renderer::render_to_texture` submit (vello owns its own
encoder + queue.submit, see [vello_tile_rasterizer.rs:175-187](../netrender/src/vello_tile_rasterizer.rs#L175-L187)).
The consumer's `present_frame` does ≤1 additional submit, and may
batch its copies with other GPU work it owns (UI overlay,
non-netrender content) for 0 incremental submits in practice.

Cost-shape vs. [§2.4 of the rasterizer plan](2026-05-01_vello_rasterizer_plan.md#L224-L239):

| Property | Today (post-7') | Path (b′) | (a) for reference |
| --- | --- | --- | --- |
| Submits per frame | 1 (vello) | 1 (vello) + ≤1 (consumer copy, batchable) | N (vello, per tile) |
| Native compositor handoff | None | **Yes** (per declared surface) | Yes (per tile) |
| Cross-frame GPU work skipping | None | **Partial** (clean surfaces skip blit) | Yes (clean tiles' textures reused) |
| Damage granularity | Tile-grid (internal) | Surface (consumer-defined) | Tile-grid |

---

## 2. API surface

Both the trait and the types live in **`netrender_device::compositor`**
— same crate as `WgpuHandles`, no dependency on `netrender`. This
keeps the consumer-facing surface thin (one crate import) and
avoids the cycle that `netrender → netrender_device` would create
if types reached back across the boundary.

### 2.1 `Compositor` trait — `netrender_device::compositor`

**Compile-ready** (drop into `netrender_device/src/compositor.rs`):

```rust
use crate::core::WgpuHandles;

/// Stable, consumer-supplied identifier for a compositor surface.
/// Survives across frames; the consumer owns the keyspace.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SurfaceKey(pub u64);

/// One compositor surface's per-frame present payload. The
/// consumer maps each `SurfaceKey` to a native texture it owns
/// (IOSurface / DXGI / etc.) and routes it to the OS compositor.
pub struct LayerPresent {
    pub key: SurfaceKey,
    /// Where this surface's pixels live within the master texture
    /// passed to `present_frame`. Always in master pixel space.
    pub source_rect_in_master: [u32; 4],
    /// World-to-screen transform the OS should apply at present
    /// time. Distinct from any transform internal to the master.
    pub world_transform: [f32; 6],  // 2D affine, column-major
    pub clip: Option<[f32; 4]>,
    pub opacity: f32,
    /// `true` ↔ netrender repainted `source_rect_in_master` this
    /// frame OR the surface is newly declared / bounds changed /
    /// returning after absence. The consumer ORs in its own
    /// "destination texture reallocated" concern when deciding
    /// whether to skip the blit.
    pub dirty: bool,
}

pub struct PresentedFrame<'a> {
    /// Single texture netrender rendered the frame into. Lifetime
    /// tied to the `present_frame` call. wgpu 29's `Texture` is
    /// internally Arc-shared, so the consumer may `clone()` if it
    /// needs to keep the handle past return for async blit work.
    pub master: &'a wgpu::Texture,
    /// Same wgpu primitives the embedder originally handed to the
    /// renderer. Provided so the consumer can encode + submit
    /// copies during `present_frame` without re-acquiring its own
    /// handles.
    pub handles: &'a WgpuHandles,
    /// One entry per declared surface, in scene declaration order.
    /// Declaration order is the surface z-order — first declared
    /// is bottom-most. Consumer hands native textures to the OS
    /// compositor in this order.
    pub layers: &'a [LayerPresent],
}

pub trait Compositor {
    fn declare_surface(&mut self, key: SurfaceKey, world_bounds: [f32; 4]);
    fn destroy_surface(&mut self, key: SurfaceKey);
    /// Called once per `Renderer::render_with_compositor`. The
    /// consumer is responsible for any GPU copies from
    /// `frame.master` into its own per-surface native textures
    /// (skipping clean surfaces), and for handing those textures
    /// to the OS compositor.
    fn present_frame(&mut self, frame: PresentedFrame<'_>);
}
```

### 2.2 Scene API — `netrender::scene`

**Illustrative-signature-only** (final field positions / serde
attrs etc. land at implementation):

```rust
/// One declared compositor surface. Order in the storage vec is
/// z-order: first declared is bottom-most. Same convention as
/// `Scene::ops` (painter order = vec order). Surface re-declare
/// updates in place — does not reorder.
pub struct CompositorSurface {
    pub key: SurfaceKey,
    pub bounds: [f32; 4],
    pub transform: [f32; 6],   // 2D affine, defaults to identity
    pub clip: Option<[f32; 4]>,
    pub opacity: f32,           // defaults to 1.0
}

impl Scene {
    /// Declare or update a compositor surface. If the key was not
    /// present, append to the surface list (z-order = position).
    /// If present, update fields in place (no reorder). Surfaces
    /// are about *cross-frame OS handoff regions*; layers
    /// (`SceneOp::PushLayer`) are about *within-frame compositing
    /// groups*. They compose without interaction.
    pub fn declare_compositor_surface(&mut self, surface: CompositorSurface);

    pub fn undeclare_compositor_surface(&mut self, key: SurfaceKey);

    /// Mutate one surface's transform/clip/opacity without
    /// touching bounds (which would force a repaint-region
    /// recompute). For OS-level animation that doesn't change
    /// painted content.
    pub fn set_surface_transform(&mut self, key: SurfaceKey, transform: [f32; 6]);
    pub fn set_surface_clip(&mut self, key: SurfaceKey, clip: Option<[f32; 4]>);
    pub fn set_surface_opacity(&mut self, key: SurfaceKey, opacity: f32);
}
```

**Storage on `Scene`:** `Vec<CompositorSurface>` — declaration
order = z-order. Re-declare updates in place. Do **not** use a
`HashMap`: layer order matters when surfaces overlap, and a hash
map's iteration order is unstable.

### 2.3 Renderer entry point — `netrender::Renderer`

**Compile-ready signature:**

```rust
impl Renderer {
    /// Render `scene` into an internal master texture of the
    /// given format, then hand declared compositor surfaces to
    /// `compositor` via `present_frame`. Per-call mode selection
    /// — same Renderer, three entry points (`render`,
    /// `compose_into`, `render_with_compositor`); the consumer
    /// picks per call.
    ///
    /// `master_format` MUST match the format of the
    /// consumer-owned destination textures: `copy_texture_to_texture`
    /// requires identical formats unless a render-graph format
    /// conversion is added (out of scope for this plan).
    /// `Rgba8Unorm` is the default for graphshell-shaped consumers;
    /// `Bgra8Unorm` is typical for native-compositor paths
    /// (CALayer / DXGI Composition).
    pub fn render_with_compositor(
        &mut self,
        scene: &Scene,
        master_format: wgpu::TextureFormat,
        compositor: &mut dyn Compositor,
        base_color: vello::peniko::Color,
    ) -> Result<(), vello::Error>;
}
```

Rationale for per-call (vs. constructor-time): same rasterizer
state survives across calls, the consumer chooses output mode at
the moment of render. No flag day for adopters; switching modes
is mechanical.

---

## 3. Render-path swap

**Today** ([vello_tile_rasterizer.rs:166-187](../netrender/src/vello_tile_rasterizer.rs#L166-L187)):

```text
Scene
 → tile_cache.invalidate → dirty TileCoord list
 → rebuild dirty per-tile vello::Scenes (clean ones reused)
 → compose master vello::Scene (push_layer per tile + Scene::append)
 → vello::Renderer::render_to_texture(target_view)
```

**Path (b′)**:

```text
Scene
 → tile_cache.invalidate → dirty TileCoord list  (now retained, not collapsed to count)
 → rebuild dirty per-tile vello::Scenes (clean ones reused)
 → compose master vello::Scene (unchanged)
 → vello::Renderer::render_to_texture(internal_master_texture)   [vello's submit]
 → compute per-surface dirty bits from
     (tile_intersection_dirty
      || newly_declared
      || bounds_changed
      || absent_last_frame)
 → build LayerPresent vec in scene declaration order
 → compositor.present_frame(PresentedFrame { master, handles, layers })
       └─ consumer encodes + submits its own copies (skipping clean surfaces)
          consumer hands native textures to the OS compositor
```

Existing master compose ([line 308-358](../netrender/src/vello_tile_rasterizer.rs#L308-L358))
is unchanged. The new work happens after `render_to_texture`
returns: per-surface dirty computation + the present_frame call.
**netrender does not encode any copies itself** — the consumer
owns the destination textures and the encoder/submit cadence.

The `internal_master_texture` is owned by the rasterizer and
pool-allocated by `(width, height, format)` — reused frame to
frame, reallocated only on viewport resize or format change.

[vello_tile_rasterizer.rs:245-246](../netrender/src/vello_tile_rasterizer.rs#L245-L246)
needs to retain the dirty `Vec<TileCoord>` instead of collapsing
it to `last_dirty_count`. Either store both, or store the Vec and
compute the count on demand.

---

## 4. Per-surface dirty tracking

Four OR-able sources, all maintained inside the rasterizer (none
by the consumer except the implicit "I reallocated my destination
texture" case, which the consumer handles by ignoring `dirty:
false` and copying anyway):

| Source | Maintained as | Computation |
| --- | --- | --- |
| `tile_intersection_dirty` | per-frame from invalidate | `dirty_tiles.iter().any(\|c\| tile_world_rect(c).intersects(surface.bounds))` |
| `newly_declared` | event flag set on `declare_compositor_surface` | clear after one frame's `present_frame` |
| `bounds_changed` | per-key prev_bounds compared frame-over-frame | clear after one frame |
| `absent_last_frame` | per-frame seen-set vs. previous-frame seen-set | implicit |

**Illustrative state:**

```rust
struct CompositorState {
    /// Per-key state held across frames.
    seen_last_frame: HashSet<SurfaceKey>,
    prev_bounds: HashMap<SurfaceKey, [f32; 4]>,
    /// Cleared after one frame's present.
    pending_declares: HashSet<SurfaceKey>,
}
```

Implementation note: bounding-box test against dirty tiles is
O(N_dirty × N_surfaces) per frame, both typically small (<10
dirty tiles, <5 compositor surfaces on graphshell or smolweb).
**Don't pre-build a spatial index.** If profiling shows it
matters at scale, add one then.

The reviewer's case "destination texture reallocated" is handled
consumer-side: under option (B) the consumer owns the destination
textures, knows when it reallocates, and can copy regardless of
netrender's `dirty` bit. netrender's `dirty: bool` is the
*content-side* signal; consumer ORs in *target-side* signals
locally.

---

## 5. Implementation order

Sub-phases land independently; each green-tests before the next.

### 5.1 — Scaffolding (no behavior change) — **SHIPPED**

- New module `netrender_device::compositor` with the trait,
  `SurfaceKey`, `LayerPresent`, `PresentedFrame` types. **No
  `netrender` import.** Only `crate::core::WgpuHandles`, `wgpu`,
  scalar types.
- `Scene::declare_compositor_surface` / `undeclare` / setter API;
  storage `Vec<CompositorSurface>` on `Scene`.
- Empty `Renderer::render_with_compositor` that delegates to
  `render` against an internal master texture (pool-allocated by
  `(width, height, master_format)`) and calls
  `compositor.present_frame` with `layers = &[]`. Verifies
  plumbing without any per-surface logic.

**Done condition:** new entry point compiles, `render` path
unchanged, master-texture pool allocates and reuses across
frames at the same dimensions.

### 5.2 — Per-surface dirty tracking — **SHIPPED**

- Retain `Vec<TileCoord>` from `tile_cache.invalidate(scene)`
  instead of collapsing to a count.
- Add `CompositorState` (per-key seen-set, prev_bounds,
  pending_declares) inside `VelloTileRasterizer`.
- `Renderer::render_with_compositor` computes `LayerPresent` per
  declared surface with `dirty` correctly OR-ed from all four
  sources. `world_transform` / `clip` / `opacity` defaults pulled
  from `CompositorSurface` storage.

**Done condition:** stub `Compositor` test impl asserts that
across two `render_with_compositor` calls with the same Scene,
all surfaces report `dirty: false` on the second call, AND a
re-declared surface (different bounds) reports `dirty: true` on
the call after re-declaration even when scene content hasn't
changed.

### 5.3 — Master texture handoff — **SHIPPED**

- Master texture allocated as
  `wgpu::TextureUsages::COPY_SRC | RENDER_ATTACHMENT`, format =
  the `master_format` parameter.
- `present_frame` invocation receives `PresentedFrame { master,
  handles, layers }`.
- **netrender does not encode any GPU copies** — the consumer's
  `Compositor` impl is responsible for encoding
  `copy_texture_to_texture` from `frame.master[layer.source_rect_in_master]`
  into its own per-surface destination textures, using
  `frame.handles.device` and `frame.handles.queue`, skipping
  layers where `dirty: false`.

**Done condition:** receipt test —
`p13prime_path_b_blit_dirty_only`. Two surfaces declared, only
one's bounds painted dirty in frame 2, the stub Compositor's
recorded copy list contains only the dirty surface's blit
(consumer-side accounting).

### 5.4 — Surface transform / clip / opacity wiring — **SHIPPED**

- Setters on `Scene` (`set_surface_transform`, etc.) update the
  matching `CompositorSurface` in place without touching the
  per-key `prev_bounds` state — these mutations don't change
  paint regions, only metadata reaching `LayerPresent`.
- Receipt: a surface with non-identity transform is correctly
  reflected in `LayerPresent.world_transform`; the master
  pixels are unrotated (the OS compositor does the rotation);
  setting transform alone does **not** flip `dirty: true`.

### 5.5 — serval adapter (separate workspace, separate commit)

- Implement `Compositor` against serval's compositing layer.
- Native-texture lifecycle managed by serval (per-platform
  glue lives there, not in netrender).
- Reshape the rendering-context surface to feed
  `render_with_compositor` instead of today's
  `present`-target-view path.

This sub-phase is the load-bearing reshape on the consumer side
and lives in the `serval` repo, not here.

#### Cut milestone vs done condition

5.5 is large enough that it needs an explicit two-step framing,
otherwise consumers conflate "the path compiles and a master
texture flows" with "every platform presents through its native
compositor."

**Cut milestone (5.5a) — sufficient to call the integration "real" — ✅ SHIPPED:**

- `Compositor` impl exists on the serval side
  (`ServoCompositor` in `components/paint/compositor.rs`); the master
  texture reaches it through `Renderer::render_with_compositor`.
- A capturing fallback (`WgpuMasterCaptureBackend`, renamed from the
  earlier `StubCompositor`) is the default; embedders that don't
  install a per-platform backend still see a populated master.
- At least one per-platform backend exists with working construction
  and per-frame body — Windows DXGI Composition (`WindowsDxgiBackend`)
  is the reference; macOS (`MacosCALayerBackend`) and Linux
  (`WaylandSubsurfaceBackend`) skeletons compile.
- `Paint::render` actually drives the renderer + compositor (was a
  stub during the C3 cut).

**Done condition (5.5b) — required to flip D3 to ✅ — 🟡 parity tail open:**

- Per-`SurfaceKey` declared-compositor-surfaces work — `frame.layers`
  is iterated in `ServoCompositor::present_frame`, per-layer
  destination wgpu textures are allocated lazily, the master rect is
  copied via `copy_texture_to_texture` (wgpu 29 `TexelCopyTextureInfo`
  shape), and `OsCompositorBackend::declare/present/destroy` wires
  through to the OS surface. ✅ landed in
  `paint: D3.5b — ServoCompositor::present_frame iterates layers`.
- Per-platform default install — `default_compositor_for_window`
  (and the `_or_capture` fallback variant) in
  `components/paint/compositor_factory.rs` cfg-dispatches to
  `WindowsDxgiBackend` / `MacosCALayerBackend` /
  `WaylandSubsurfaceBackend` wrapped in `ServoCompositor`, falling
  back to `WgpuMasterCaptureBackend` on unknown platforms (offscreen
  embedder routes, headless tests, etc.). ✅ landed in
  `paint: D3.5b — default_compositor_for_window factory`.
- End-to-end test that drives `Paint::render` directly (not just
  the renderer + compositor in isolation) —
  `tests/paint_render_e2e.rs` covers the full embedder path:
  `Paint::new_for_test` → `install_renderer` →
  `handle_messages(SendDisplayList)` → `render(webview_id)` →
  `composite_texture(painter_id)`. ✅ 3/3 passing. Landed in
  `paint: D3.5b — Paint::render e2e integration test`.
- Each per-platform backend has its per-frame body wired and a smoke
  receipt. macOS is real for both master and declared surfaces via
  `--macos-present-surfaces-smoke`. Windows is real for the DCOMP
  composition swapchain master path via `--windows-present-smoke`, but
  still needs the per-`SurfaceKey` DCOMP `present` override and a
  matching `--windows-present-surfaces-smoke`. Linux still needs a live
  Wayland session. **Remaining gap.**

---

## 6. Scope estimate

| Sub-phase | Lines (production + test) |
| --- | --- |
| 5.1 Scaffolding | ~120 |
| 5.2 Per-surface dirty | ~150 (up from ~100 — four-source OR + state struct) |
| 5.3 Master handoff | ~80 (down from ~150 — netrender no longer encodes copies) |
| 5.4 Transform/clip/opacity | ~80 |
| 5.5 serval adapter | TBD (lives in serval) |

netrender side: **~430 lines + tests** — under the original §13'
deferral estimate of 600-1000.

---

## 7. Receipts

Each sub-phase ships with at least one receipt under `tests/`:

- `p13prime_path_b_present_plumbing` (5.1) — empty layers list
  reaches stub compositor; master texture pool allocates once and
  reuses across frames.
- `p13prime_path_b_dirty_clean_after_unchanged` (5.2) — same
  scene, two calls; second reports all surfaces clean.
- `p13prime_path_b_dirty_on_bounds_change` (5.2) — bounds change
  with no painted-content change reports `dirty: true` on the
  next frame.
- `p13prime_path_b_blit_dirty_only` (5.3) — two surfaces, only
  one dirty; stub Compositor records exactly one copy.
- `p13prime_path_b_transform_only_clean` (5.4) — setting
  transform between frames updates `LayerPresent.world_transform`
  but reports `dirty: false`.
- `p13prime_path_b_zorder_preserved` (5.3) — three overlapping
  surfaces declared in known order; `LayerPresent` slice order
  matches declaration order.

Platform-level integration tests (CALayer / DXGI / Wayland) live
in serval, not netrender. netrender's job is the trait
surface and the master texture handoff.

---

## 8. Open questions (non-blocking)

1. **Master texture format defaults vs. consumer choice.** The
   `master_format` parameter on `render_with_compositor` accepts
   any vello-storage-compatible format. `Rgba8Unorm` is the
   default and works on stock `REQUIRED_FEATURES` (empty).
   Native-compositor paths often want `Bgra8Unorm` as the
   destination, but **`Bgra8Unorm` is not directly usable as the
   master**: vello's GPU compute path requires `STORAGE_BINDING`
   on its target, and BGRA8 storage requires the
   `BGRA8_UNORM_STORAGE` wgpu feature which is not in
   netrender's [`REQUIRED_FEATURES`](../netrender_device/src/core.rs#L17).
   Two paths forward, decided at 5.3 implementation time:
     - Add `BGRA8_UNORM_STORAGE` to `REQUIRED_FEATURES` (modest
       widening — most modern adapters support it).
     - Keep master as `Rgba8Unorm`, consumer adds a format-
       converting blit shader between master and BGRA destination.
   `copy_texture_to_texture` requires source/dest format match,
   so cross-format copies need a render-graph blit pass either
   way. Tied to roadmap F1 (HDR/wide-gamut) for wider gamut work.

2. **Surface bounds outside viewport.** Declared surface bounds
   may extend past the master rect (consumer's CSS positioning).
   Decision: clamp `source_rect_in_master` to master bounds; the
   consumer's native texture handles the rest as transparent.
   Document but don't validate at API level.

3. **Backdrop filter interaction.** 12c' slices the master at
   backdrop-filter indices for its own multi-pass dance. Path
   (b′) reads from the *final* master, after backdrop filters
   have been incorporated. They compose without interaction.

4. **Async copy lifetimes.** `&'a wgpu::Texture` in
   `PresentedFrame` is borrow-bound to `present_frame`. wgpu 29's
   `Texture` is `Clone` (Arc-internal), so consumers needing to
   queue copies for later submission can `texture.clone()` and
   keep the handle past `present_frame` return. Document this in
   the trait method's doc comment so consumers know the pattern.

5. **Vello encoder batching as a future optimization.** vello
   currently owns its `render_to_texture` encoder and submits
   internally. If linebender/vello later exposes a
   `render_to_encoder(&mut wgpu::CommandEncoder)` API, netrender
   could batch the master render with consumer copies into a
   single submit. Watch upstream; not blocking.

---

## 9. What this kills

- The `v1.5 fallback` in [§2.4 of the rasterizer plan](2026-05-01_vello_rasterizer_plan.md#L224-L239)
  ("whole-frame vello + post-render tile slicing for native-
  compositor handoff") — superseded by this plan, which is
  strictly better (per-surface damage instead of flat slicing).
  Mark §2.4 as superseded when 5.3 lands.
- The "(a) vs (b)" framing in the originally-deferred §13' —
  this plan supersedes that dichotomy. The deferred-phases
  doc has already been rewritten to reflect the activation.
- The sentence in [rasterizer §13 risk #8](2026-05-01_vello_rasterizer_plan.md#L1895-L1900)
  ("for browser-content scrolling workloads at large viewports
  the regression is real and would only be addressed by
  forking") — partially recovered. Path (b′) gives back
  cross-frame GPU-work skipping at the *surface* granularity,
  not the *tile* granularity. Still not full webrender-style
  sub-tile damage, but closer.
