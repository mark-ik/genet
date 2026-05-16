# serval — cut plan (C1.5 — C7)

Companion to the C1 commit (`651a83b62cd`, *cut GL/surfman corpus
from rendering-context layer*). Captures the imposed shape this fork
is moving toward, the cuts left after C1, and the contract each
cut ends at.

C1.5–C4 are the **netrender cut** (renderer becomes netrender-driven,
webrender gone). C5–C7 started as the **script-optional cut**, but the
target has sharpened: Serval should compile as a profile ladder from a
Blitz-like static HTML shape up to fullweb, not as one hard browser
composition with runtime no-ops. The canonical C5/C7 implementation plan
now lives in
[2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md).
C5 and C7 remain continuity labels here, but their detailed work is
spun out into that plan.

Pattern (per the netrender bring-up that succeeded): **rip the
parallel codepath, fix what breaks, don't try to incrementally
migrate**. Each cut is "delete the corpus, run cargo check,
resolve the holes."

---

## Cut status snapshot (2026-05-12)

- **C1** — ✅ landed pre-session. GL/surfman corpus removed.
- **C1.5** — ✅ landed. WebGL deletion: 45 DOM files + 35 WebIDLs +
  bindings + canvas surface; `gleam`/`glow`/`mozangle`/`surfman`
  out of workspace; WebXR confirmed opt-in stub.
- **C1.6** — ✅ landed pre-session. Pelt shell root + engine-profile
  seam.
- **C2** — ✅ landed. `paint_types` extraction; `components/paint/`
  impl deleted; layout `webrender_api` migration completed via
  C3 reshape.
- **C3** — ✅ landed (2026-05-08). Layout reshape, paint_info
  plumbing through `PaintMessage::SendDisplayList`, and Step 7
  painter (`translate_display_list` + per-pipeline `Scene`s +
  3 passing unit tests). `cargo check -p servo-layout` clean.
  See [2026-05-08_c3_landed_notes.md](./2026-05-08_c3_landed_notes.md).
- **C4** — ✅ code-complete for Windows/macOS compositor parity
  (2026-05-12).
  `ServoCompositor` adapter + shared `present_frame` routing are in
  tree; `paint_render_e2e` drives `Paint::render` end to end (3/3
  passing on Windows), and `default_compositor_for_window` is
  cfg-gated. macOS has both the master CAMetalLayer path and the
  per-`SurfaceKey` CALayer/IOSurface path validated by `pelt
  --macos-present-surfaces-smoke`. Windows now has the DXGI
  Composition master path plus per-`SurfaceKey` child-visual path in
  code: `WindowsDxgiBackend::declare` creates the DCOMP child visual,
  transform, clip, and composition swapchain; `present` copies the
  keyed destination into that swapchain, applies transform/clip/opacity,
  presents it, and commits. The matching Pelt smoke is
  `--windows-present-surfaces-smoke`. Linux
  `WaylandSubsurfaceBackend` remains externally gated on a live Wayland
  session. The prior 20 `Paint`-method gaps in
  `components/servo/webview.rs` and the missing
  `paint_api::rendering_context*` imports in `components/servo/lib.rs`
  are closed in the C4 tail.
- **C5** — ⏸ spun out. Now covered by profile-ladder P1–P3:
  split layout host services from script messages, remove script from
  `layout` / `layout_api`, and add a static HTML document provider.
- **C6** — ✅ code complete. `ScriptingProfile` + NoOp factories.
- **C7** — ⏸ spun out. Now covered by profile-ladder P5–P6:
  introduce profile facade packages and keep the low-profile document
  pipeline separate from the fullweb constellation path.

Validation baseline (2026-05-12): `cargo check -p pelt-desktop
--features windows-present` clean; `cargo check -p pelt --features
windows-present` clean; `cargo check -p pelt-desktop --features
windows-present --target x86_64-pc-windows-msvc` clean; `cargo check
-p pelt --features windows-present --target x86_64-pc-windows-msvc`
clean. All checks still report the pre-existing `servo-paint`
dead-code warnings for `paint_info` and `next_painter_id`. The headed
Windows per-surface smoke now runs to completion on the Windows desktop
session (`cargo run -p pelt --features windows-present -- --engine
viewer --windows-present-surfaces-smoke about:blank`, outcome:
`frames=2 created_window=true declared_subsurface=true`) and was
visually confirmed with a red master surface plus a green top-left
declared DCOMP child visual. The Windows smoke requests an 800x600
physical client area so the DCOMP master and declared child visual cover
the intended surface on DPI-scaled displays. Linux still needs a live
Wayland smoke.

---

## Next work lanes (2026-05-12)

### Lane 1 — Windows per-surface presentation parity

**Status:** landed in code on 2026-05-12; headed Windows smoke runs to
completion and the declared child visual was visually confirmed.

**Goal:** make Windows match macOS for declared compositor surfaces,
not just the master/full-window path.

**Why first:** C4 is otherwise easy to overstate. macOS proves both
`present_master` and per-`SurfaceKey` `present`; Windows only proves
the DXGI Composition swapchain master path today. Closing this before
the profile ladder keeps renderer parity separated from profile/facade
work.

**Work landed:**

- `ServoCompositor::present_frame` now submits all layer blits before
  invoking backend per-surface `present`, so a backend that copies the
  destination during `present` observes the freshly-written texture.
- `WindowsDxgiBackend::declare` allocates a COPY_DST/COPY_SRC wgpu
  destination, creates a DCOMP child visual, attaches a composition
  swapchain as that visual's content, installs a matrix transform and
  rectangle clip, and inserts the child above the master root visual.
- `WindowsDxgiBackend::present` applies transform/clip/opacity, copies
  the keyed destination into the child swapchain backbuffer, presents
  that swapchain, and commits the DCOMP tree.
- `WindowsDxgiPresentSmokeConfig::declare_subsurface` plus Pelt
  `--windows-present-surfaces-smoke` mirror the macOS smoke: red
  master, green top-left declared surface, 50% opacity, window held
  open for visual confirmation.

**Receipt:** `pelt --windows-present-surfaces-smoke` launches the
Windows DCOMP declared-surface path, exits cleanly, and shows the green
declared surface composited above the red master:

```bat
cargo run -p pelt --features windows-present -- --engine viewer --windows-present-surfaces-smoke about:blank
```

### Lane 2 — Serval profile ladder

**Goal:** resume the script-optional work as a profile-ladder
implementation after Windows/macOS compositor parity is honest. The
profile ladder makes Serval compile from `serval-static-html` through
`serval-fullweb`; it does not bolt Blitz onto a separate engine.

**Order:**

1. Update this snapshot and the C4 landed notes once Lane 1 lands.
2. Start profile-ladder P0/P1: add dependency gates and split layout
   host services from `ScriptThreadMessage`.
3. Complete the old C5 through profile-ladder P2/P3: remove `script`
   from layout/layout_api, then add the static HTML document provider.
4. Reconfirm C6 remains useful after the profile split. It should select
   runtime factories, but must not hide a compile-time hard dependency on
   `script`.
5. Complete the old C7 through profile-ladder P5/P6: add profile facade
   packages and keep the low-profile document pipeline separate from the
   fullweb constellation path.

See
[2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md)
for the detailed slice plan, package graph, gates, and pitfalls.

**Deferred / externally gated:** macOS GPU-side per-surface sync can
wait for upstream `wgpu-hal` queue access if it becomes necessary;
Linux Wayland presentation needs hardware/session coverage that is not
available on the current X11-only Linux box.

---

## The imposed shape

```text
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

## Windows validation note

For narrow migration slices, prefer crate checks that do not reach
`components/servo` / `components/script` when possible. Examples:
`cargo check -p servo-paint-api`, `cargo check -p servo-canvas-traits`,
`cargo check -p servo-webxr`, and similar leaf or shared crates. These
avoid the SpiderMonkey native build path and keep iteration cheap.

The old ServoShell browser launcher was different: `components/servo`
depends on `servo-script`, and `servo-script` depends on the `js` /
`mozjs_sys` stack. That route is no longer the active shell validation
root for Serval/Pelt work.

Pelt validation should stay on the script-free path:

```bat
cd /d C:\Users\mark_\Code\repos\serval
cargo check -p pelt
cargo run -p pelt -- --engine viewer --netrender-smoke about:blank
```

Only use the native SpiderMonkey setup when deliberately testing the old
browser-engine crate graph. The least-painful Windows path observed for
that route is:

```bat
"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\LaunchDevCmd.bat" -arch=x64
cd /d C:\Users\mark_\Code\repos\serval
set LINKER=link.exe
set HOST_LINKER=link.exe
cargo clean -p mozjs_sys
cargo check -p servo
```

Notes:

- Do not start with a full `cargo clean`; the workspace target directory
  can be very large. Use `cargo clean -p mozjs_sys` only after changing
  the native build environment.
- Servo's Windows bootstrap expects repo-local MozTools at
  `target/dependencies/moztools/4.0`. `mach bootstrap` should populate
  that path; a direct Cargo workflow can also use the Servo build-deps
  `moztools-4.0.zip` package in that layout.
- Visual Studio must include the C++ build tools, Windows SDK, and ATL
  component. Running from the Build Tools developer prompt avoids
  `mozjs_sys` accidentally selecting a Visual Studio instance without
  ATL headers.
- If a slice needs the old browser crate graph, treat that as a
  deliberate engine test, not as Pelt validation.

---

## C1.6 — Pelt shell root and engine-profile seam

**Why:** the shell root is **Pelt**, not ServoShell. Pelt is the place
for windows, input, tabs, dialogs, prefs, webdriver command routing,
protocol UI, and platform integration. The old all-up Servo browser
launcher is not retained as the active compatibility target; keeping it
would preserve the exact GL/JS/browser coupling this cut is meant to
break.

**Current landed scaffold:**

- `ports/pelt-core/` defines `EngineProfile`, `ShellEngine`,
  capability reporting, and deferred `viewer` / `static` / `headless`
  profiles.
- `ports/pelt-desktop/` is the destination crate for winit/platform
  windows, input translation, native dialogs, filesystem integration,
  and platform event-loop glue. It now owns the script-free static
  viewer loop and creates a real winit window.
- `ports/pelt-ui-egui/` is the destination crate for chrome/tabs/
  location/dev UI. Its renderer backend is wgpu-only; there is no
  `egui_glow`, `chrome-glow`, or GL compatibility lane.
- `ports/pelt/` is the active package, library, and binary. The
  workspace default member is `ports/pelt`, and the old `ports/servoshell`
  path has been removed from the active workspace.
- The `pelt` default feature set is `viewer-netrender` +
  `chrome-wgpu`. It does not depend on the all-up `servo` facade,
  `servo-script`, `mozjs_sys`, `egui_glow`, or GL window chrome.
- The Pelt/NetRender lane disables default `wgpu` backend features and
  enables native `dx12` / `metal` / `vulkan` / `wgsl` explicitly. This
  keeps `glow`, GLES, EGL, and WGL helper crates out of the active Pelt
  Cargo tree.
- `pelt --engine browser` is rejected. Browser becomes a future engine
  adapter decision, not a preserved launcher root.
- `cargo check -p pelt` compiles the script-free entrypoint without
  building `servo-script` or `mozjs_sys`.
- `cargo run -p pelt -- --engine viewer --netrender-smoke about:blank`
  boots NetRender through the script-free Pelt desktop lane,
  renders a 64x64 `netrender::Scene` through `Renderer::render_vello`,
  reads pixels back, and then runs the same first-redraw window loop.
  Current receipt: `painted_pixels=4096`, `created_window=true`,
  `redraws=1`.

**What this does not solve yet:** Pelt still only proves NetRender with
offscreen readback plus a separate winit redraw. It does not present the
NetRender output into the viewer window yet, and it does not provide a
browser engine adapter.

**Next cut:** move the remaining browser-owned window/webview state into
the Pelt crates, register shell protocols in the viewer profile, load
static resources, and present the NetRender output in the actual viewer
window instead of proving it only with offscreen readback.

**Done condition for the next cut:** `pelt --engine viewer <static-url>`
presents visible static document pixels through the netrender/wgpu path.

### C1.6 operating map

**Where we are:** Pelt is a script-free shell lane, not a browser
compatibility wrapper. It has a real workspace root, a real winit window
smoke, a wgpu-only chrome crate boundary, and a NetRender offscreen
paint/readback receipt. This is enough to validate shell/platform/render
work without touching `components/servo`, `components/script`, or
SpiderMonkey.

**Where we are headed:** the next proof is visible pixels in the Pelt
window. The viewer profile should create a wgpu surface for the winit
window, render a simple NetRender scene into the swapchain target, and
then grow from a hardcoded scene into static URL/resource loading. Browser
support comes later as an engine adapter decision, not as the root shell
identity.

**Fruitful sidequests:**

- Add a small Pelt validation command/script that runs `cargo check -p
  pelt`, the NetRender smoke, and a `cargo tree -p pelt` denylist for
  `servo-script`, `mozjs`, `glow`, `surfman`, `webrender`, and
  `egui_glow`.
- Sweep stale `ServoShell` naming in product metadata, docs, and comments
  once the Pelt crates are committed.
- Make `ShellEngineCapabilities` drive chrome decisions, even while the
  viewer profile reports most capabilities as false.
- Keep `pelt-ui-egui` contract-shaped until presentation is real; chrome
  polish should not outrun the render path.
- Add a presented-frame screenshot/readback receipt once NetRender draws
  into the viewer window.

**Pitfalls:**

- Do not rebuild the old browser under the Pelt name. Early imports of
  `servo::WebView`, `ServoBuilder`, concrete browser delegates, or
  `script_traits` collapse the seam.
- Do not treat offscreen readback as presentation. It proves NetRender can
  paint, not that Pelt can present.
- Do not let default `wgpu` features sneak GLES/GL helper crates back into
  the Pelt lane.
- Do not make `browser` the default profile again. Browser is a future
  adapter, not the active shell root.
- Do not make SpiderMonkey setup part of ordinary Pelt validation. It is
  only for deliberate old-browser-graph checks.
- Do not forget that the new Pelt crates must be tracked in git; untracked
  shell crates make every validation result easy to lose.

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

**Status (2026-05-06):** C3 paint-side scaffold landed —
`components/paint/` is a compile-clean `NetrenderPainter` stub
([components/paint/netrender_painter.rs](../components/paint/netrender_painter.rs))
with the public API surface servo.rs consumes. 11 WebRender-wrapper
impl files (~5605 LOC) deleted; new scaffold ~145 LOC. Method bodies
are no-ops or `unimplemented!()` for action paths. Layout-side
reshape is the next cut — see
[2026-05-06_c3_layout_reshape_plan.md](./2026-05-06_c3_layout_reshape_plan.md)
for the focused plan covering the 5 broken layout files (124 errors)
and the real `NetrenderPainter` translator implementation.

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

**Status (2026-05-12):** shared C4 plumbing is landed; macOS has
master + per-`SurfaceKey` smoke coverage; Windows has the master DCOMP
path plus per-`SurfaceKey` child-visual code and matching
`--windows-present-surfaces-smoke`; Linux still needs an on-device
Wayland smoke receipt. See
[2026-05-09_c4_landed_notes.md](./2026-05-09_c4_landed_notes.md).
The direction-neutral interop primitives the per-platform backends
build on top of are documented in
[2026-05-09_interop_lineage.md](./2026-05-09_interop_lineage.md)
(slint → graft → scrying → serval lineage; explains why the
import-direction `InteropSynchronizer` trait was dropped on the
export side).

**Done condition (cut milestone — D3.5a, ✅):** `Compositor` impl
exists on the serval side; the master texture reaches it through
`Renderer::render_with_compositor`. A capturing fallback
(`WgpuMasterCaptureBackend`, formerly `StubCompositor`) is the
default; embedders that don't install a per-platform backend still
see a populated master. At least one per-platform backend exists
with working construction (Windows DXGI Composition is the
reference). `Paint::render` actually drives the renderer +
compositor (was a stub during the C3 cut).

**Done condition (full — D3.5b, 🟡):** A `<div>` renders into a
serval-owned native texture; on macOS, a CALayer presents that texture;
on Windows, a DXGI Composition Visual; on Linux, a Wayland subsurface.
Per-`SurfaceKey` declared compositor surfaces work when `frame.layers`
is iterated and each layer's native handle is routed via
`OsCompositorBackend::present`. Per-platform default install —
`default_compositor_for_window` factory dispatches by
`cfg(target_os = …)`, falling back to the capturing backend on unknown
platforms or via the `_or_capture` variant. End-to-end test that drives
`Paint::render` directly (`paint_render_e2e_drives_full_embedder_path`)
passes on Windows. macOS is green for declared surfaces via
`--macos-present-surfaces-smoke`; Windows has the equivalent
per-surface DCOMP body plus a clean headed
`--windows-present-surfaces-smoke` run and visual confirmation of the
green child visual above the red master; Linux still needs a live
Wayland session.

**Scope:**

- Trait + ServoCompositor + WgpuMasterCaptureBackend: ~300 lines.
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

## C5 — Cut `script` dep from `components/layout`

**Status:** spun out into
[2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md).

C5 is still the continuity label for cutting script out of layout, but
the implementation is no longer just "replace imports until
`cargo check -p layout` works." The sharpened target is a Serval static
HTML profile whose package graph can lay out and paint HTML/CSS without
`script`, `script_traits`, `script_bindings`, or `mozjs`.

The new C5 shape is profile-ladder P1–P3:

- **P1:** replace `LayoutConfig::script_chan:
  GenericSender<ScriptThreadMessage>` with a profile-neutral layout host
  service trait.
- **P2:** remove `script` / `script_traits` from
  `components/layout` and `components/shared/layout`, and move
  `ScriptThreadFactory` out of `layout_api`.
- **P3:** add a static HTML document provider that parses with
  `html5ever`, implements the layout input contract, and renders through
  `servo-layout` -> `servo-paint` -> NetRender.

**Done condition:** `cargo check -p servo-layout` succeeds without
script in its normal library graph, and `cargo check -p
serval-static-html` proves the Blitz-like minimum Serval profile.

---

## C6 — Route `ScriptThread::create` through profile-typed factory

**Why:** the constellation pipeline spawn surface is already generic
([components/constellation/pipeline.rs:75](../components/constellation/pipeline.rs#L75) —
`fn spawn<STF: ScriptThreadFactory, SWF: ServiceWorkerManagerFactory>`),
but the concrete picker is hardcoded at
[components/servo/servo.rs:1314](../components/servo/servo.rs#L1314)
(`script::ScriptThread::create(...)`). C6 makes that picker
profile-driven so `EngineProfile::Viewer` doesn't need a working
`ScriptThread` to start a pipeline.

**Cuts:**

- The concrete `script::ScriptThread::create` call site in
  `components/servo/servo.rs`. Delete the import + concrete call.
- Replace with profile-typed dispatch (illustrative-signature-only):

```rust
let stf: Box<dyn ScriptThreadFactory> = match profile {
    EngineProfile::Browser  => Box::new(BrowserScriptFactory),
    EngineProfile::Viewer   |
    EngineProfile::Static   => Box::new(ViewerNoOpFactory),
    EngineProfile::Headless => Box::new(HeadlessFactory),
};
constellation.spawn(stf, swf, ...);
```

- New `ViewerNoOpFactory` impl that produces a script-free pipeline
  (no DOM mutation, no JS, no service workers).

**The shape change:** the engine profile from `pelt-core` reaches
the constellation spawn site, and which factory runs is a profile
decision rather than a Cargo decision.

**Done condition:** under `EngineProfile::Viewer`, the constellation
spawns pipelines without instantiating `script::ScriptThread`.
`cargo check -p servo` (browser composition) still succeeds with
the same behavior as before.

**Scope:** ~200–500 LOC (factory enum, viewer no-op impl, dispatch
edits at the call site). Single-day cut. Can land before or after the
profile-ladder P1/P2 work, but it only becomes meaningful once layout no
longer pulls script.

**Deferred decisions:**

- **Where the factory enum lives** — `pelt-core` (engine-profile-
  aware), `components/constellation` (locality of dispatch), or a
  new `components/engine_factory/` crate. Pick at scaffold time.
- **`ServiceWorkerManagerFactory` in viewer profile** — likely
  no-op too; service workers require script. Decide alongside.
- **Headless factory shape** — separate from viewer because
  headless is automation-shaped (webdriver), not document-viewer-
  shaped. Defer until headless profile is real.

---

## C7 — Cut `script` dep from `components/servo`

**Status:** spun out into
[2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md).

C7 is still the continuity label for cutting script out of the all-up
facade, but the target is now stricter than "`cargo check -p servo
--no-default-features` somehow works." The target is named profile
packages whose dependency graphs prove that browser/fullweb is one
composition, not Serval's minimum identity.

The new C7 shape is profile-ladder P5–P6:

- **P5:** introduce profile facade packages such as
  `serval-static-html` and `serval-fullweb`, keeping the default
  fullweb/browser behavior intact while giving the static profile its
  own dependency witness.
- **P6:** keep the low-profile document pipeline direct at first
  (`parse -> layout -> paint -> NetRender`) instead of forcing static
  HTML through the script-heavy `components/constellation` path.

**Done condition:** the static profile package checks without
`script`, `script_bindings`, or `mozjs`; the existing default `servo`
fullweb/browser build still checks and behaves as before.

---

## What stays untouched across C1.5–C7

- `components/script/` — content side. Tons of `webrender_api`
  imports today; these are C2 sed targets but the script logic
  itself doesn't change. Untouched by the profile-ladder work except as
  a fullweb provider of the extracted layout/script contracts.
- `components/layout/` — imports change (C2 paint-types, profile-ladder
  P1/P2 shared layout contract work), but layout algorithms remain
  unchanged.
- `components/canvas/` 2D path — kept (only the WebGL canvas
  variants die in C1.5).
- `components/webgpu/` — independent of C1.5/C2; might reference
  webrender_api types for ExternalImageId etc. (C2 migrates
  those).
- `components/constellation/`, `components/script_bindings/`,
  `components/net/`, `components/storage/`, etc. — fullweb components
  stay intact. The profile ladder should let lower profiles bypass them
  before it tries to reshape them.
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
