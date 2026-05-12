# C4 — landed notes

C4 (the netrender `Compositor` adapter, OS-handoff backends, and
the `Paint::render` path that drives both) is landed, with one
Windows parity tail still open. This doc captures what shipped under
the cut-milestone (D3.5a) and done-condition (D3.5b) framing, the
validation receipts, and what remains before C4 is universally green.

Companion to:

- [2026-05-05_serval_netrender_cut_plan.md](./2026-05-05_serval_netrender_cut_plan.md) — overall cut plan
- [2026-05-08_c3_landed_notes.md](./2026-05-08_c3_landed_notes.md) — C3 (translator + per-pipeline scenes), C4's prerequisite
- [2026-05-05_compositor_handoff_path_b_prime.md](./2026-05-05_compositor_handoff_path_b_prime.md) — netrender's path-(b′) design + the shared 5.5a/5.5b framing
- [2026-05-09_interop_lineage.md](./2026-05-09_interop_lineage.md) — provenance of the direction-neutral interop primitives (slint → graft → scrying → serval) consumed by the C4 backends

---

## Why two milestones

5.5 is large enough that consumers conflate "the path compiles
and a master texture flows" with "every platform presents through
its native compositor." The cut plan splits the work so the
status snapshot can stay honest:

- **Cut milestone (D3.5a)** — `Compositor` impl exists, master
  flows through `Renderer::render_with_compositor`, the
  capturing fallback is the default, at least one per-platform
  backend is real with its per-frame body wired, and
  `Paint::render` actually drives the renderer instead of
  stubbing.
- **Done condition (D3.5b)** — `frame.layers` is iterated and
  routed to the OS compositor; `default_compositor_for_window`
  picks the right backend per `cfg`; an end-to-end test drives
  `Paint::render` (not just renderer + compositor in isolation);
  per-platform backends each have a live smoke receipt.

D3.5a and the shared D3.5b plumbing are landed. macOS now has both
master and per-`SurfaceKey` smoke coverage. Windows has the DCOMP
master path, but its per-`SurfaceKey` `present` body still inherits
the trait default no-op and needs the matching Pelt smoke mode.
Linux remains platform-bound on a live Wayland session.

---

## What landed

### D3.5a — cut milestone

| File | Change |
| --- | --- |
| [components/paint/compositor.rs](../components/paint/compositor.rs) | `WgpuMasterCaptureBackend` (renamed from `StubCompositor`; deprecated alias retained), `OsCompositorBackend` trait, `ServoCompositor<B>` wrapper holding a `HostWgpuContext` + per-`SurfaceKey` destination texture pool. Default `present_master` no-op for embedder-route capture; per-platform backends override. |
| [components/paint/compositor_dxgi.rs](../components/paint/compositor_dxgi.rs) | `WindowsDxgiBackend`. DXGI Composition swapchain, master `IDCompositionVisual`, `IDCompositionTarget` rooted at the embedder HWND, and per-`SurfaceKey` visual bookkeeping. Master `present_master` is wired; per-`SurfaceKey` `present` remains the parity tail. |
| [components/paint/compositor_calayer.rs](../components/paint/compositor_calayer.rs) | `MacosCALayerBackend`. Constructor accepts an `NSView`/`UIView` raw pointer, attaches a `CAMetalLayer`, imports the drawable into wgpu for master presentation, and allocates per-surface IOSurface/CALayer destinations. Master and per-`SurfaceKey` paths are smoke-tested on macOS. |
| [components/paint/compositor_wayland.rs](../components/paint/compositor_wayland.rs) | `WaylandSubsurfaceBackend` skeleton. Constructor accepts `wl_display` + `wl_surface` raw pointers, allocates per-`SurfaceKey` `wl_subsurface`. Per-frame body declared but unverified — needs Linux smoke. |
| [components/paint/interop/](../components/paint/interop/) | Direction-neutral foundation extracted from `wgpu-native-texture-interop` patterns: `HostWgpuContext`, `InteropBackend`, `SyncMechanism`, `InteropError`, `Dx12FenceSynchronizer` (Windows-only). Per-platform synchronizers live as inherent impls — no import-direction-coupled trait. WNTI itself was finally cut from the dep graph during D3.5b cleanup (it had been a stale `paint_api` `wgpu_backend` feature flag carryover from pre-C3 work; nothing in `components/` or `ports/` ever imported it). |
| [components/paint/netrender_painter.rs](../components/paint/netrender_painter.rs) | `Paint::render(webview_id)` walks `webview_to_pipeline` → `pipelines[pid].scene` → `renderer.render_with_compositor(scene, format, &mut compositor, base)`. `Paint::composite_texture(painter_id)` reads through `WgpuMasterCaptureBackend::last_master`. `install_compositor` accepts `Box<dyn PaintCompositor>`; trait upcasting (rustc 1.86+) lets `&mut **compositor` flow into `Renderer::render_with_compositor`. |

### D3.5b — shared done-condition plumbing

| Commit | Change |
| --- | --- |
| `paint: D3.5b — ServoCompositor::present_frame iterates layers` | Per-`SurfaceKey` destination wgpu textures allocated lazily, sized to `source_rect_in_master`; `backend.declare/destroy` fires on (re)alloc; `copy_texture_to_texture` encodes the master→layer copy via wgpu 29's `TexelCopyTextureInfo` shape; submit goes through `frame.handles.queue`; `backend.present(key, transform, clip, opacity)` is called for each layer. Encoder is single-shot per frame; only submitted when at least one layer was dirty. macOS overrides `present`; Windows still needs the DXGI override. |
| `paint: D3.5b — default_compositor_for_window factory` | New [components/paint/compositor_factory.rs](../components/paint/compositor_factory.rs). `default_compositor_for_window(host, display, window) -> Result<Box<dyn PaintCompositor>, BoxedFactoryError>` cfg-dispatches to `WindowsDxgiBackend` / `MacosCALayerBackend` / `WaylandSubsurfaceBackend` wrapped in `ServoCompositor`, falling back to `WgpuMasterCaptureBackend` on unknown platforms. `default_compositor_for_window_or_capture(...)` logs and falls back instead of erroring — for embedders that just want pixels. Adds `raw-window-handle` to servo-paint deps. |
| `paint: D3.5b — Paint::render e2e integration test` | New [components/paint/tests/paint_render_e2e.rs](../components/paint/tests/paint_render_e2e.rs). Three tests covering the embedder-facing path the production loop walks: full success (`paint_render_e2e_drives_full_embedder_path`), unknown-webview no-op (`paint_render_unknown_webview_is_noop`), and per-frame master replacement (`paint_render_replaces_captured_master_per_frame`). Adds two test-only constructors on `Paint`: `new_for_test()` (skips `InitialPaintState`, uses `NoopWaker` + unbounded crossbeam channel + dummy `CrossProcessPaintApi`) and `install_renderer(painter_id, renderer)` (sidesteps `register_rendering_context`'s `WgpuCapability` path so the test can inject a `netrender::Renderer` built from `boot()` / `create_netrender_instance`). |

---

## Validation receipts

- `cargo test -p servo-paint --test paint_render_e2e` — 3/3 pass
  on Windows (Vulkan via wgpu's auto-selected backend; D3D12 also
  works when forced).
- `cargo test -p servo-paint --test c4_smoke_probe` — still
  green; was the renderer + compositor isolation probe (D3.5a),
  now superseded by `paint_render_e2e` for embedder-path coverage
  but kept as a tighter regression net.
- pelt `--windows-present-smoke about:blank` — DCOMP composition
  swapchain present validates the Windows master OS-handoff body
  end-to-end (swapchain → IDCompositionVisual → desktop). This does
  not yet exercise per-`SurfaceKey` DCOMP visuals.

Validation env at [`.cargo-check-logs/cargo-check-env.ps1`](../.cargo-check-logs/cargo-check-env.ps1)
(clang-cl + `-utf-8` + NASM + MOZILLABUILD + VS 2022 vcvars).

---

## Remaining gaps before C4 is universally ✅

0. **Windows per-surface DCOMP parity.** `WindowsDxgiBackend::declare`
  already creates an `IDCompositionVisual` per `SurfaceKey` and stores
  it with the destination texture, but `WindowsDxgiBackend` still
  inherits the trait default no-op for `present(key, transform, clip,
  opacity)`. Wire that body so the per-surface visual is attached under
  the root, displays the declared surface destination, receives the
  transform/clip/opacity state, and commits through DCOMP. Add a Pelt
  `--windows-present-surfaces-smoke` mode mirroring the macOS smoke
  (red master, green declared surface, 50% opacity) so parity has a
  visible receipt.

1. **macOS smoke receipt — ✅ landed (2026-05-09).**
   `MacosCALayerBackend::new` now constructs end-to-end (extracts
  `MTLDevice` via wgpu-hal, attaches `CAMetalLayer` to the
  embedder NSView's CALayer with frame matched + autoresizing,
  contentsScale inherited for HiDPI). `present_master` syncs
  `drawableSize`, imports `nextDrawable.texture` into wgpu, runs the
  master -> drawable `TextureBlitter` copy on the same queue as
  netrender's submit, and calls `[drawable present]`. The
  per-`SurfaceKey` `declare`/`destroy`/`present`
   paths are also wired: `declare` allocates an `IOSurface` (RGBA8
   FourCC `'RGBA'`), wraps as an `MTLTexture` via
   `newTextureWithDescriptor:iosurface:plane:`, hands to wgpu via
   `Device::create_texture_from_hal::<Metal>` (pure objc2 path —
   no `metal-rs`), creates a per-surface `CALayer` with
   `contents = IOSurface`, and adds it as a sublayer; `present`
   applies `transform`/`clip`/`opacity` to the per-surface
   `CALayer`. Visual receipt: `pelt --macos-present-surfaces-smoke`
   shows the per-surface CALayer correctly compositing at 50%
   opacity over the master CAMetalLayer (olive blends where master
   red shows through, pure green where master green is occluded).
2. **macOS `CAMetalLayer.pixelFormat` documented contract
   violation — ✅ landed (2026-05-09).** Master path on macOS now
   uses `BGRA8Unorm` for the `CAMetalLayer.pixelFormat` (on
   Apple's documented allow-list) with a per-frame
   `wgpu::util::TextureBlitter` doing the `Rgba8Unorm` master ->
   `Bgra8Unorm` drawable format conversion. Vello's master stays
   `Rgba8Unorm` (its compute pipeline's hardcoded storage-binding
   format); vello's own
   [`render_to_texture`](https://github.com/linebender/vello/blob/02c2501/vello/src/lib.rs#L474)
   doc recommends `TextureBlitter` for exactly this conversion at
   the surface boundary. Bonus side-effect: the previous
   `device.poll(Wait)` CPU stall is gone — `TextureBlitter::copy`
   runs on wgpu's queue (same queue netrender's vello submits to),
   so the master->drawable copy is naturally FIFO-ordered against
   vello's render submit; `[drawable present]` (the no-arg
   `MTLDrawable::present`) waits on the drawable's pending GPU
   writes per Apple's docs, sidestepping the wgpu-hal queue
   accessor block listed in (3) for the master path. The own-
   `MTLCommandQueue` + `MTLSharedEvent` plumbing is removed.
   Per-`SurfaceKey` IOSurface destinations stay `Rgba8Unorm` —
   `CALayer.contents = IOSurface` reads bytes per the IOSurface's
   declared format and 'RGBA' is supported there in practice.

3. **macOS GPU-side cross-queue sync — deferred unless needed.** The
  master path no longer needs a CPU stall: the drawable texture is
  imported into wgpu, `TextureBlitter::copy` runs on the same queue as
  netrender's vello submit, and `[drawable present]` waits for pending
  GPU writes to the drawable. Future per-`SurfaceKey` sync upgrades may
  still want upstream wgpu-hal access to the underlying Metal queue
  (for example a `raw_queue()` accessor mirroring
  `Device::raw_device()`), but that no longer blocks the visible smoke
  path.
4. **Linux smoke receipt.** `WaylandSubsurfaceBackend` is still a
   skeleton — `wl_subsurface` placement + commit, `dmabuf` import
   path need a Wayland session (Mutter or Sway) to validate.
5. **C4 tail — `components/servo/webview.rs` `Paint`-method
   gaps — ✅ landed (closed prior to this doc revision).** Every
   method `webview.rs` calls on `Paint` (`add_webview`,
   `remove_webview`, `render`, `composite_texture`,
   `register_rendering_context`, `resize_rendering_context`,
   `set_hidpi_scale_factor`, `show_webview`, `hide_webview`,
   `notify_scroll_event`, `notify_input_event`, `set_page_zoom`,
   `page_zoom`, `adjust_pinch_zoom`, `pinch_zoom`,
   `device_pixels_per_page_pixel`, `capture_webrender`,
   `request_screenshot`, `toggle_webrender_debug`) now exists in
   [components/paint/netrender_painter.rs](../components/paint/netrender_painter.rs).
   The previously-flagged `paint_api::rendering_context*` imports
   in [components/servo/lib.rs:49,54](../components/servo/lib.rs)
   are present and re-export cleanly. `cargo check -p servo` on
   the Rust type-check level passes against the netrender Paint;
   the remaining `cargo check -p servo` cost on Mac is the
   SpiderMonkey native build, not Rust-side method gaps.

(0) is the active Windows/macOS parity lane. (1) is ✅. (2) is ✅ as
well. (3) was a wait-and-see deferred to (2)'s landing — the
master-path sync is now FIFO-ordered without the wgpu-hal queue
accessor; future per-`SurfaceKey` GPU sync upgrades may still want it,
but no longer block anything visible. (4) gates D3 ✅ on Linux. (5) is
✅. Once (0) lands, the remaining roadmap should move to C5/C7 while
Linux stays externally gated.

---

## Architectural notes worth carrying forward

### Trait upcasting buys back the `Box<dyn PaintCompositor>` ergonomics

`Paint::install_compositor` takes `Box<dyn PaintCompositor>`, but
`Renderer::render_with_compositor` wants `&mut dyn Compositor`.
With rustc 1.86+'s trait upcasting, `&mut **compositor` flows
from one to the other without an explicit `dyn-to-dyn` cast
(via `PaintCompositor: Compositor`). This is what lets the
factory return one boxed trait object that satisfies both
contracts. If we ever need to support an MSRV before 1.86, the
adapter is a one-liner inside `Paint::render`.

### Direction-neutral interop foundation, no WNTI dep

C4's `OsCompositorBackend` consumes its interop primitives from
[`crate::interop`](../components/paint/interop/), authored fresh
for the export direction. The decision not to depend on
[`wgpu-graft/wgpu-native-texture-interop`](../../wgpu-graft/wgpu-native-texture-interop/)
(WNTI) was deliberate — WNTI's `InteropSynchronizer` trait is
import-direction-coupled (`producer_complete(&NativeFrame)` /
`consumer_ready(&ImportedTexture)`), and patching the trait or
fabricating dummy `NativeFrame`s to satisfy the signature would
both be uglier than mirroring the small direction-neutral
foundation locally. Per-platform synchronizers (e.g.
`Dx12FenceSynchronizer`) expose **inherent** methods that the
backend impl calls directly — no import-direction wrapping.

### `WgpuMasterCaptureBackend` is not a stub — it's a real route

Renamed from `StubCompositor` in D3.5a precisely because the name
was misleading. This is the wgpu-shared-device embedder route:
the embedder holds the same wgpu device as netrender, so the
master texture it reads via `composite_texture` is directly
samplable in its own render pass (zero copy). The right backend
when the embedder wants serval's composite as an input to its
own pipeline (custom UI shell, scene composition); the wrong one
when the embedder wants serval to present pixels directly to the
OS (then install a per-platform backend via the factory).

### Cut milestone vs done condition is reusable framing

D3.5a / D3.5b worked well as a way to keep the integration shape
honest while platform-specific work was still in flight. Worth
applying the same split to future cuts where the "this works
end-to-end" claim is measured per-platform — C5 (script-cut from
layout) and C7 (script-cut from servo facade) are likely
candidates: cut milestone = "compiles without script in the
graph for one composition," done condition = "every composition
the cut promises actually runs."
