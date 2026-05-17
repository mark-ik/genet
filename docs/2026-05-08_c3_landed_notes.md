# C3 — landed notes

C3 (the netrender layout-reshape + painter scaffold) is done. This
doc captures what shipped, the translator coverage, the validation
recipe (which has Windows-specific moving pieces), and the entry
conditions for C4.

Companion to:

- [archive/2026-05-05_serval_netrender_cut_plan.md](./archive/2026-05-05_serval_netrender_cut_plan.md) — overall cut plan (archived 2026-05-17)
- [archive/2026-05-06_c3_layout_reshape_plan.md](./archive/2026-05-06_c3_layout_reshape_plan.md) — the C3 layout-reshape plan that was executed (archived 2026-05-17)

---

## Done condition

- `cargo check -p servo-layout` — clean (0 errors, 15 unused-import warnings).
- `cargo check -p servo-paint` — clean (1 unused-field warning).
- `cargo test -p servo-paint` — 3/3 translator tests pass.
- `cargo check -p servo` — passes through layout cleanly; remaining 20 errors are in `components/servo/webview.rs`, all `Paint` method gaps, which is C4 territory.

The plan's done condition was *"`cargo check -p servo` reaches script through layout cleanly"* and *"synthetic display list renders to Scene"* — both met.

---

## What landed

### Layout-side reshape (Step 3+4+6)

| File | Change |
|---|---|
| [components/layout/display_list/conversions.rs](../components/layout/display_list/conversions.rs) | `ToWebRender` impls retargeted to `paint_types` + `paint_api::serval_display_list::FilterOp`. New `FilterOp` shape (single-arg `Blur`/`Opacity`, struct-variant `DropShadow`). |
| [components/layout/display_list/mod.rs](../components/layout/display_list/mod.rs) | Local `wr` shim re-exports paint-side types under historical names. `DisplayListBuilder::build` returns `ServalDisplayList`. `webrender_api::*` direct refs replaced. `CommonItemPlacement` uses webrender field names (`clip_rect`/`spatial_id`/`clip_chain_id`). |
| [components/layout/display_list/stacking_context.rs](../components/layout/display_list/stacking_context.rs) | Imports retargeted; `FilterOp::Opacity` arity dropped 2 → 1; `ReferenceFrameKind` simplified to unit variants (optimization-hint fields deferred). |
| [components/layout/display_list/background.rs](../components/layout/display_list/background.rs), [gradient.rs](../components/layout/display_list/gradient.rs) | Imports + body retargets. `builder.wr().create_*_gradient(...)` replaced with direct `GradientPayload` construction. |
| [components/layout/display_list/hit_test.rs](../components/layout/display_list/hit_test.rs) | Picks up `BoxCorners` trait (added to paint_types::units). |
| [components/layout/style_ext.rs](../components/layout/style_ext.rs) | `wr::PrimitiveFlags` → `paint_api::serval_display_list::PrimitiveFlags`. |
| [components/layout/layout_impl.rs](../components/layout/layout_impl.rs) | Imports retargeted; empty + main `send_display_list` paths use `ServalDisplayList`; `paint_info` plumbed through (was a FIXME); `send_initial_transaction` removed in favor of lazy painter-side init. |

### Shared paint-side surface

| File | Change |
|---|---|
| [components/shared/paint/lib.rs](../components/shared/paint/lib.rs) | `PaintMessage::SendDisplayList` carries `paint_info: PaintDisplayListInfo` alongside the `display_list`. `CrossProcessPaintApi::send_display_list` takes 3 args. |
| [components/shared/paint/serval_display_list.rs](../components/shared/paint/serval_display_list.rs) | ~30 webrender-shaped compat methods on `ServalDisplayList` (`push_rect` / `push_image` / `push_text` / `push_border` / `push_box_shadow` / `push_gradient` / `push_iframe` / `push_hit_test` / `push_stacking_context` / `push_reference_frame` / `define_clip_rect` / `define_clip_rounded_rect` / `define_clip_chain` / `define_scroll_frame` / `define_sticky_frame` + `begin`/`end`/`dump_serialized_display_list` no-ops). Stub types: `ComplexClipRegion`, `Shadow`, `SpaceAndClipInfo`, `HasScrollLinkedEffect`, `WrClipId`. Unified on `paint_types::SpatialId`/`PropertyBindingKey`/`PropertyValue` (re-exported as `PropertyBinding`); duplicates dropped. |
| [components/shared/paint-types/composite.rs](../components/shared/paint-types/composite.rs) | Added `MixBlendMode::PlusLighter`. |
| [components/shared/paint-types/sticky.rs](../components/shared/paint-types/sticky.rs) | Added `StickyOffsetBounds::new(min, max)`. |
| [components/shared/paint-types/units.rs](../components/shared/paint-types/units.rs) | Added `BoxCorners` trait (`top_left`/`top_right`/`bottom_left`/`bottom_right` for `Box2D`). |
| [components/geometry/lib.rs](../components/geometry/lib.rs) | `FastLayoutTransform`: `to_transform` made `pub`; added `project_point2d`, `is_backface_visible`, `pub from_transform`, `From<LayoutTransform>`. |

### Painter (Step 7)

| File | Change |
|---|---|
| [components/paint/translator.rs](../components/paint/translator.rs) | New, ~350 LOC. `translate_display_list(list, paint_info) -> netrender::Scene`. Per-variant emit helpers + 3 unit tests. |
| [components/paint/netrender_painter.rs](../components/paint/netrender_painter.rs) | `Paint` holds `pipelines: RefCell<FxHashMap<PipelineId, PipelineState>>` + `dirty_webviews`. `handle_messages` routes `SendDisplayList` through translator and stores per-pipeline `Scene` + `paint_info`. `PipelineExited` removes pipeline state. `GenerateFrame` is a stub (C4). |
| [components/paint/lib.rs](../components/paint/lib.rs) | `pub use crate::translator::translate_display_list;` |
| [components/paint/Cargo.toml](../components/paint/Cargo.toml) | Added deps: `log`, `netrender`, `paint_types`, `rustc-hash`. Dev deps: `euclid`, `servo-geometry`, `stylo_traits`. |

---

## Translator coverage

What `translate_display_list` does today, in the order each variant
appears in `ServalDisplayItem`:

| `ServalDisplayItem` | `netrender::SceneOp` (or other) | Notes |
|---|---|---|
| `Rect` | `SceneRect` via `push_rect` | Full coverage. |
| `RectWithAnimation` | `SceneRect` via `push_rect` | Static color (animation hook deferred — `paint_info.caret_property_binding` is consulted but not advanced per-frame yet). |
| `Line` | `SceneRect` (degenerate) | Wavy/dashed/dotted line decorations deferred. |
| `Image` | (logged + skipped) | Needs painter-side `ImageRegistry` to map `paint_types::ImageKey` → `netrender::ImageKey`. |
| `RepeatingImage` | (logged + skipped) | Translator path is `ScenePattern` via `Scene::push_pattern`; same ImageKey wiring needed. |
| `Text` | (logged + skipped) | Needs painter-side `FontRegistry` to map `FontInstanceKey` → `netrender::FontId`. |
| `Border` | 4 × `SceneRect` (per side) | First-cut edge-strokes from `widths`. Per-side styles (Solid/Dashed/Dotted) and rounded corners deferred. NinePatch logged + skipped. |
| `BoxShadow` | (logged + skipped) | Needs `Renderer::build_box_shadow_mask` — the painter must hold a `Renderer` handle (C4 territory). |
| `PushShadow` / `PopAllShadows` | (logged + skipped) | Same dependency as `BoxShadow`. |
| `Gradient` | `SceneGradient { kind: Linear, ... }` via `push_gradient` | N-stop gradients with arbitrary stops; works. |
| `RadialGradient` | `SceneGradient { kind: Radial, ... }` | Works (uses `radius.width × radius.height` ellipse). |
| `ConicGradient` | `SceneGradient { kind: Conic, ... }` | Works. |
| `Iframe` | (logged + skipped) | Needs cross-pipeline scene composition or `declare_compositor_surface` (C4 territory). |
| `PushStackingContext` | `SceneLayer` via `push_layer` | Opacity computed from `FilterOp::Opacity` filters; `MixBlendMode` mapped (CSS-canonical subset; ColorDodge/ColorBurn/HardLight fall back to Normal until netrender grows full coverage). |
| `PopStackingContext` | `pop_layer` | |
| `PushReferenceFrame` / `PopReferenceFrame` | (recorded; transforms palette already populated up-front) | netrender selects per-op `transform_id` rather than push/pop. |
| `HitTest` | (recorded for hit-test layer; not in paint stream) | |

What's deferred (each marked with a `warn!` in the translator):

- BoxShadow / Shadow → renderer-level (`build_box_shadow_mask`)
- Image / RepeatingImage / Text → ImageRegistry / FontRegistry wiring
- Iframe → cross-pipeline composition or compositor-surface route

---

## Validation recipe (Windows)

Reproduces in [`.cargo-check-logs/cargo-check-env.ps1`](../.cargo-check-logs/cargo-check-env.ps1).

```powershell
$env:Path = "C:\Users\mark_\AppData\Local\bin\NASM;$env:Path"
$env:CFLAGS = "-utf-8"   # dash form; msys2 mangles /utf-8 into a path
$env:CXXFLAGS = "-utf-8"
$env:CC = "clang-cl"     # mozjs 140 fmt 11.x has unprotected GCC syntax — MSVC cl.exe rejects
$env:CXX = "clang-cl"
$env:HOST_CC = "clang-cl"
$env:HOST_CXX = "clang-cl"
$env:MOZILLABUILD = "C:/mozilla-build"
cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" 1>nul 2>&1 && cargo check -p servo-layout'
```

Required VS 2022 components: **C++ MFC for v143** + **C++ ATL for v143**
(installed under VC\Tools\MSVC\14.44.35207\atlmfc\). NASM via
`winget install NASM.NASM`. MozillaBuild 4.2.1 at `C:\mozilla-build`.

If multiple VS versions are installed (e.g. VS 2026 Insiders alongside
VS 2022), `vcvars64.bat` from VS 2022 forces the 14.44 toolchain;
without it, cc-rs picks the highest-version cl.exe, and SpiderMonkey
140's headers don't compile under MSVC 14.51 (VS 2026).

Test invocation:

```powershell
cmd /c '"...vcvars64.bat" 1>nul 2>&1 && cargo test -p servo-paint'
```

---

## C4 entry conditions

C4 builds the `ServoCompositor` adapter (impl `netrender_device::Compositor`)
and the OS-handoff backends. The remaining 20 errors in
`components/servo/webview.rs` are the `Paint`-method gaps that C4 closes:

```text
add_webview, remove_webview, render, composite_texture,
register_rendering_context, resize_rendering_context,
set_hidpi_scale_factor, show_webview, hide_webview,
notify_scroll_event, notify_input_event, set_page_zoom,
page_zoom, adjust_pinch_zoom, pinch_zoom,
device_pixels_per_page_pixel, capture_webrender, request_screenshot
```

Plus the unresolved imports `paint_api::rendering_context` and
`paint_api::rendering_context_core::GlCapability` in
`components/servo/lib.rs:49,54`.

C4 design sketch is in
[archive/2026-05-05_serval_netrender_cut_plan.md § C4](./archive/2026-05-05_serval_netrender_cut_plan.md#c4--build-servocompositor-adapter)
(archived 2026-05-17).

---

## Interop foundation — extracted into `crate::interop`

C4's `OsCompositorBackend` consumes its direction-neutral interop
primitives from a new in-tree module
[`components/paint/interop/`](../components/paint/interop/), authored
fresh for the export direction.

The choice between depending on
[`wgpu-graft/wgpu-native-texture-interop`](../../wgpu-graft/wgpu-native-texture-interop/)
(WNTI) and extracting was deliberate. WNTI's surface is
direction-neutral at the data level (`InteropBackend`,
`HostWgpuContext`, `SyncMechanism`) but its `InteropSynchronizer`
trait is import-coupled — `producer_complete(&NativeFrame)` and
`consumer_ready(&ImportedTexture)` take parameter shapes that don't
exist in the export direction. To use WNTI we'd either patch its
trait (forbidden — WNTI is not getting reshaped to fit serval) or
fabricate dummy `NativeFrame` values to satisfy the signature. Both
are worse than mirroring the small direction-neutral foundation in
serval.

The extracted surface:

| Type | Source |
|---|---|
| `InteropBackend { Vulkan, Metal, Dx12, Unknown }` | [`interop/mod.rs`](../components/paint/interop/mod.rs) |
| `HostWgpuContext { device, queue, backend }` + `detect_backend()` | [`interop/mod.rs`](../components/paint/interop/mod.rs) |
| `SyncMechanism { None, ExplicitExternalSemaphore, ExplicitFence }` | [`interop/mod.rs`](../components/paint/interop/mod.rs) |
| `InteropError { BackendMismatch, Dx12, Vulkan, Metal, UnsupportedSynchronization }` | [`interop/mod.rs`](../components/paint/interop/mod.rs) |
| `Dx12FenceSynchronizer { new, shared_handle, advance, current_value, queue_wait }` (Windows only) | [`interop/windows_dx12.rs`](../components/paint/interop/windows_dx12.rs) |

What's *missing* compared to WNTI — and why that's fine:

- No `InteropSynchronizer` trait. The
  [`OsCompositorBackend`](../components/paint/compositor.rs) trait
  itself owns per-frame fence dance; per-platform synchronizers
  (e.g. `Dx12FenceSynchronizer`) expose inherent methods that the
  backend impl calls into directly. No import-direction wrapping.
- No `NativeFrame` / `ImportedTexture` / `WgpuTextureImporter` /
  `vulkan_dmabuf` / `raw_gl` / `surfman_gl` / `ProducerCapabilities`.
  These are import-side only; serval has no use for them.
- No `thiserror` dep — `InteropError` impls `Display` + `Error` by
  hand (the enum is small).

Serval's `components/paint` does not import any WNTI symbol; only
the `windows` crate (Windows-only target dep) is added directly.
Re-exports surface from
[`components/paint/lib.rs`](../components/paint/lib.rs):
`HostWgpuContext`, `InteropBackend`, `InteropError`, `SyncMechanism`,
`OsCompositorBackend`, `ServoCompositor`, `StubCompositor`,
`Dx12FenceSynchronizer` (cfg `windows`).

(Note added 2026-05-09: at C3 time, `paint_api`'s `wgpu_backend`
feature still listed `wgpu-native-texture-interop` as an optional
dep, which `components/paint`'s `paint_api` activation pulled into
the lockfile transitively. No Rust source touched it; the dep was
removed during D3.5b cleanup. See
[2026-05-09_c4_landed_notes.md](./2026-05-09_c4_landed_notes.md).)

## Known follow-ups (post-C4)

These were flagged during the C3 work; capturing here so they're not
lost.

- **Animation property bindings**: only `caret_property_binding` is in
  flight today. The translator reads `paint_info.caret_property_binding`
  but does not advance the animation per frame; needs a tick callback
  on `Paint`.
- **Reference-frame transform `kind`**: webrender's `Transform`
  variant carries `is_2d_scale_translation` / `should_snap` /
  `paired_with_perspective` optimization hints; the netrender
  translator doesn't consume them yet. Simplifying to unit variants
  in `paint_types::ReferenceFrameKind` was a deliberate first-cut.
- **MixBlendMode coverage**: ColorDodge/ColorBurn/HardLight/SoftLight/
  Difference/Exclusion/Hue/Saturation/Color/Luminosity/PlusLighter
  fall back to `SceneBlendMode::Normal` until netrender's enum grows.
- **Wavy / dashed / dotted line decorations**: `Line` items render as a
  solid degenerate rect in the first cut.
- **Per-side border styles + rounded corners**: 4 edge-strokes only.
- **NinePatch borders**: logged + skipped.
- **`servo-layout` warnings**: 15 unused imports survive in the layout
  files; `cargo fix --lib -p servo-layout` cleans them. Held back to
  keep the diff scope narrow.
