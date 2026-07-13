# Wayland per-surface presentation — implementation design

Status: design approved 2026-06-03; implementation pending.

Companion to:

- [`docs/2026-05-28_wayland_per_surface_presentation_gap.md`](./2026-05-28_wayland_per_surface_presentation_gap.md) — the handoff brief this design implements against.
- [`docs/2026-05-09_c4_landed_notes.md`](./2026-05-09_c4_landed_notes.md) — gap (4) is the work scoped here.
- [`docs/2026-05-09_interop_lineage.md`](./2026-05-09_interop_lineage.md) — the Linux synchronizer slot this design fills.
- [`docs/archive/2026-05-05_compositor_handoff_path_b_prime.md`](./archive/2026-05-05_compositor_handoff_path_b_prime.md) — path-(b′) design context.

## 1. Goal

Finish the last C4 platform gap. Land a `WaylandSubsurfaceBackend` that presents netrender's wgpu master texture and per-`SurfaceKey` declared compositor surfaces through the Wayland compositor on Linux, with both a headless smoke (`--wayland-present-surfaces-smoke` exits 0 with `declared_subsurface=true`) and a visual color receipt (red master, green declared quarter at 50% opacity, olive blend) executed on the Fedora 44 + GNOME/Mutter validation box. Clears C4/D3 on Linux; flips gap (4) in `2026-05-09_c4_landed_notes.md` to ✅.

## 2. Out of scope

- `wp_linux_drm_syncobj_v1` explicit-sync per-frame protocol (idiomatic but additive — implicit sync via `wl_buffer.release` is what mainstream Wayland clients use and what this design ships).
- X11 backend.
- Cross-process semaphore handoff via `OPAQUE_FD` (the wrapper exposes it; no consumer wired this session).
- Rotation in netrender's `world_transform` for the smoke scene (the bake path handles it correctly when it arrives; the smoke doesn't exercise it).
- Replacing the parent `wl_surface`'s xdg-shell role management — winit continues to own configure ack.

## 3. Current state of the surrounding code

The orchestration is already built. `ServoCompositor::present_frame` (`components/paint/compositor.rs:319`) iterates `frame.layers`, allocates per-`SurfaceKey` destination textures through `backend.declare`, blits `master[rect] → dest` via `copy_texture_to_texture`, and calls `backend.present(key, transform, clip, opacity)`. The Wayland backend only implements the `OsCompositorBackend` trait methods — no glue rewiring required.

The skeleton at `components/paint/compositor_wayland.rs` carries:
- Trait-shape lock (`WaylandSubsurfaceBackend::new(host, *mut wl_display, *mut wl_surface)`).
- FIXME-documented per-frame steps.
- `BackendError` with `WrongBackend` / `NullDisplay` / `NullSurface` / `Unwired` variants.
- `present_master` returning `BackendError::Unwired`.
- Default trait `declare` / `present` (no per-`SurfaceKey` handoff yet).

Companion infra already shipped:
- `compositor::OsCompositorBackend` trait + `ServoCompositor<B>::present_frame` blit loop.
- `compositor_factory::default_compositor_for_window` dispatches to `WaylandSubsurfaceBackend` on Linux from raw-window-handle's Wayland display + window handles.
- `interop::{HostWgpuContext, InteropBackend, SyncMechanism, InteropError}` + `Dx12FenceSynchronizer` (Windows). macOS + Linux synchronizer slots flagged "Pending" in `interop_lineage.md`.

Recent landings the design relies on:
- macOS per-`SurfaceKey` IOSurface + CALayer path (the closest structural analog for per-surface OS-native allocation + wgpu wrapping).
- `ServoCompositor` "OR target-side (re)alloc into the dirty signal" (so a freshly-allocated dest doesn't present uninitialized on a clean frame).
- `OsCompositorBackend` trait — backend owns destination allocation.

The 2026-05-28 brief was authored *after* all of the above; nothing in the 84 commits between the user's previous genet state and now invalidates its scope. The macOS "sync upgrade flagged as upstream-blocked" entry (`a518ecaae85`) is **macOS-specific** (`metal::Queue::raw_queue()` not exposed); Vulkan's `hal_queue.as_raw()` is exposed and supports this work.

## 4. High-level architecture

### 4.1 Topology

- **Master** attaches directly to the embedder's parent `wl_surface` (winit owns it; winit doesn't render into it). Matches the skeleton's literal FIXME.
- **Per-`SurfaceKey`** lives on a fresh `wl_surface` parented as a `wl_subsurface` of the parent, `set_desync` so subsurface commits apply on their own commit without depending on parent commit ordering.

Trade-off accepted: cross-surface atomicity is best-effort, not strict — master and per-keys commit independently. At smoke cadence (60Hz) this is invisible. The alternative (deferred parent commit acting as an end-of-frame barrier) would require adding a `commit_frame()` hook to `OsCompositorBackend` and changing every backend; not justified for the perceptible-flicker risk on a 60Hz steady state.

### 4.2 Per-frame sync model

Implicit sync via `wl_buffer.release` events — what every mainstream Wayland-dmabuf client does (GTK, KDE, Firefox). The kernel's dmabuf implicit-fence model handles GPU→compositor ordering through the dma_fence attached to the dmabuf at attach time; the compositor signals `wl_buffer.release` once it's done sampling, the client recycles the slot.

`sync_mechanism()` returns `SyncMechanism::None`. Same-queue FIFO covers the smoke path; the synchronizer wrapper (Section 4.5) is dormant.

### 4.3 Master and per-key dest are exportable VkImages

wgpu textures allocated via `Device::create_texture` aren't dmabuf-exportable. The dance: allocate the `VkImage` + `VkDeviceMemory` directly via `ash` with the right `Vk*ExternalMemory*` extension chain (Section 4.4), then wrap back into a `wgpu::Texture` via `Device::create_texture_from_hal::<Vulkan>`. wgpu treats the resulting handle indistinguishably from a self-allocated one. Same technique macOS uses with `IOSurface → MTLTexture → wgpu::Texture`; not a "macOS parallel," just the standard "wgpu-native handle bridge" the platform calls for.

### 4.4 Bake path for opacity fallback + rotation

`wp_viewporter` expresses crop + scale only — no rotation. `wp_alpha_modifier_v1` expresses per-surface opacity — available on Mutter 47 (Fedora 44), but not universal. For either case, a single per-surface render pipeline can pre-bake the result into a sibling exportable VkImage that *can* be attached as a wl_buffer directly:

- Fast path (no rotation, alpha-modifier protocol available): viewporter + alpha-modifier + per-key dest attached directly. Zero extra GPU work.
- Baked path (rotation OR alpha-modifier unavailable with non-1.0 opacity): per-surface render-pass writes source-dest through a vertex transform (linear part of the affine) and a fragment alpha multiplier into `surface.bake.exportable_image`; viewport identity-scales it; subsurface positions at the rotated bbox top-left.

The bake target is lazily allocated and reallocated only when the rotated-bbox size changes. Surfaces that never need bake never allocate it.

### 4.5 The Linux synchronizer slot

`VulkanTimelineSemaphoreSynchronizer` in `components/paint/interop/vulkan_timeline.rs`, designed in the idiomatic Vulkan-timeline style (not as a `Dx12FenceSynchronizer` look-alike):

- The semaphore handle is the API — producers integrate it into their own `VkSubmitInfo.pSignalSemaphores`/`pSignalSemaphoreValues`; consumers wire it into `pWaitSemaphores`/`pWaitSemaphoreValues`. The wrapper does not issue empty-buffer submits.
- Host-readable signaled value via `vkGetSemaphoreCounterValue`.
- Host-side wait via `vkWaitSemaphores`.
- `OPAQUE_FD` export for cross-process / external-driver consumers (real Vulkan use case, not a Dx12 shared-handle analog).

Dormant on the smoke path — its existence and constructibility fill the lineage-doc slot. When (if) a real cross-queue or cross-process consumer appears, the handle is already there.

## 5. Module structure & gated dependencies

### 5.1 File layout

Promote `compositor_wayland.rs` to a module directory (mirroring the recent `compositor_calayer/` refactor):

```
components/paint/compositor_wayland/
├── mod.rs         # WaylandSubsurfaceBackend struct + OsCompositorBackend impl
├── wayland.rs     # wayland-client connection, registry, protocol globals
├── dmabuf.rs      # ExportableImage + SurfaceBufferPool + modifier negotiation
├── bake.rs        # rotation + opacity bake pipeline (wgpu render pass)
└── errors.rs      # BackendError enum
```

New interop slot:

```
components/paint/interop/vulkan_timeline.rs  # NEW
components/paint/interop/mod.rs              # add: #[cfg(linux)] mod vulkan_timeline + re-export
```

New pelt-desktop smoke runner:

```
ports/pelt-desktop/smoke_wayland.rs          # NEW
ports/pelt-desktop/lib.rs                    # add: #[cfg(feature = "linux-present")] pub mod smoke_wayland
```

### 5.2 Dependency gating

All Linux-only deps live in a target-gated block — they do not appear in lockfile resolution on other targets.

`components/paint/Cargo.toml`:
```toml
[target.'cfg(target_os = "linux")'.dependencies]
ash = "0.38"                                 # matches wgpu-hal 29's lockfile pin (0.38.0+1.3.281)
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["client", "unstable", "staging"] }
```

`ports/pelt-desktop/Cargo.toml`:
```toml
[features]
linux-present = ["netrender", "dep:paint", "dep:raw-window-handle", "dep:pollster"]
```
No Linux-specific deps in `linux-present` — the wayland-client + ash code is encapsulated inside `paint::compositor_wayland`. The existing pelt-desktop `wgpu` dep already enables the `vulkan` feature.

`ports/pelt/Cargo.toml`: `linux-present = ["pelt-desktop/linux-present"]` pass-through.

Workspace `Cargo.toml`: no changes — keep `ash`/`wayland-*` local to paint until a second consumer materializes.

### 5.3 Source-level cfg gating

```rust
// components/paint/lib.rs
#[cfg(target_os = "linux")] pub mod compositor_wayland;

// components/paint/interop/mod.rs
#[cfg(target_os = "linux")] mod vulkan_timeline;
#[cfg(target_os = "linux")] pub use vulkan_timeline::VulkanTimelineSemaphoreSynchronizer;
```

`ports/pelt-desktop/smoke_wayland.rs` follows the macOS template — the `#[cfg(feature = "linux-present")]` outer impl with a `#[cfg(not(target_os = "linux"))]` early-return for portable `cargo check`.

`ports/pelt/viewer.rs` arg parsing + dispatch arms gated `#[cfg(feature = "linux-present")]`, mirroring the existing Windows/macOS arms.

### 5.4 Gating verification (run during landing)

- `cargo check -p servo-paint` on Linux → resolves wayland-client + ash, wayland code compiles.
- `cargo check -p servo-paint --target x86_64-pc-windows-msvc` → wayland deps invisible to Windows resolution.
- `cargo check -p pelt-desktop --features linux-present` on Linux → real impl compiles.
- `cargo tree -p pelt --target x86_64-pc-windows-msvc` → no `wayland-client`/`wayland-protocols`/`ash` lines.

## 6. Item 1 — Wayland backend per-frame body

### 6.1 Construction

`WaylandSubsurfaceBackend::new(host, *mut wl_display, *mut wl_surface)`:

1. Validate `host.backend == InteropBackend::Vulkan`; null-check both pointers.
2. `wayland_client::Connection::from_ptr(display)`. The connection borrows the display; the caller retains ownership.
3. `globals::registry_queue_init` to bind globals:

| Global | Required version | Required? |
|---|---|---|
| `wl_compositor` | ≥4 | yes |
| `wl_subcompositor` | ≥1 | yes |
| `zwp_linux_dmabuf_v1` | ≥3 (4 preferred for `get_default_feedback`) | yes |
| `wp_viewporter` | ≥1 | yes |
| `wp_alpha_modifier_v1` | ≥1 | optional |

Missing required globals → `BackendError::MissingGlobal(name)`.

4. Drain dmabuf format/modifier advertisements (v3 `format`/`modifier` events or v4 `default_feedback.tranche_format_table`). Confirm `DRM_FORMAT_ABGR8888` is advertised on at least one modifier; otherwise `BackendError::NoCompatibleFormat`.
5. Construct `VulkanTimelineSemaphoreSynchronizer` (Section 9). Dormant; held for the slot.
6. `log::info!("[WaylandSubsurfaceBackend] bound: dmabuf v{}, viewporter v{}, alpha_modifier={}", ...)` for first-run visibility.

### 6.2 Per-`SurfaceKey` state

```rust
struct WaylandSurface {
    wl_surface: WlSurface,
    wl_subsurface: WlSubsurface,
    viewport: WpViewport,
    alpha_modifier: Option<WpAlphaModifierSurfaceV1>,
    source_dest: dmabuf::SurfacePair,        // pool + ExportableImage of source-rect-sized dest
    bake: Option<dmabuf::SurfacePair>,       // lazily allocated bake target
    size: (u32, u32),                        // source-rect size
}
```

`SurfacePair` = `(ExportableImage, SurfaceBufferPool)` — the wgpu-wrapped Vulkan image plus the dmabuf wl_buffer pool drawn from it.

### 6.3 `present_master(master)`

1. `event_queue.dispatch_pending()` — drain `wl_buffer.release` events.
2. Allocate `master_side_buffer: SurfacePair` if absent or if master size changed. Source dest texture is `Rgba8Unorm`.
3. Encode `wgpu::CommandEncoder::copy_texture_to_texture(master → master_side_buffer.image.wgpu_texture)`; submit.
4. Acquire a wl_buffer slot from the pool. `parent_wl_surface.attach(buf, 0, 0)` + `damage_buffer(0, 0, w, h)` + `commit`.
5. `connection.flush()`.

### 6.4 `declare(key, host, w, h, format)`

1. Reject `format != Rgba8Unorm` (vello constraint, same as macOS).
2. Allocate `source_dest: SurfacePair` of size `(w, h)` (Section 7).
3. Create per-key `wl_surface` + `wl_subsurface` (parent = embedder parent surface, `set_desync`, `set_position(0,0)`).
4. Create `wp_viewport` from `viewporter.get_viewport(wl_surface)`.
5. If `wp_alpha_modifier_v1` was bound, create `wp_alpha_modifier_surface_v1` from `alpha_modifier.get_surface(wl_surface)`.
6. Insert `WaylandSurface { ..., bake: None }` into `surfaces`.
7. Return `source_dest.image.wgpu_texture.clone()`.

### 6.5 `present(key, transform, clip, opacity)`

1. `event_queue.dispatch_pending()`.
2. Look up `surface = surfaces.get_mut(&key)`; absent → log + return (mirrors the macOS warn-skip pattern).
3. Decide path:
   ```
   let needs_rotation = transform[1].abs() > 1e-6 || transform[2].abs() > 1e-6;
   let needs_alpha_bake = surface.alpha_modifier.is_none() && (opacity - 1.0).abs() > 1e-6;
   ```
4. **Fast path** (`!needs_rotation && !needs_alpha_bake`):
   - `viewport.set_source(0, 0, w_fixed, h_fixed)` + `viewport.set_destination(dest_w, dest_h)` derived from the transform's scale.
   - `wl_subsurface.set_position(transform[4] as i32, transform[5] as i32)`.
   - Clip: `Some([x0,y0,x1,y1])` → `wl_compositor.create_region` + `set_input_region`; `None` → `surface.set_input_region(None)`.
   - Opacity (via protocol): `alpha_modifier.set_multiplier(opacity_u32_fixed_point)`.
   - Acquire wl_buffer from `source_dest.pool`; `attach`, `damage_buffer`, `commit`, `flush`.
5. **Baked path**:
   - Compute `rotation_bbox` from the source rect under the linear affine.
   - Ensure `surface.bake` is allocated and sized to `rotation_bbox.size`; (re)allocate `SurfacePair` if not.
   - Run the bake render pass (Section 8): source = `source_dest.image.wgpu_texture`, target = `bake.image.wgpu_texture`, uniforms = linear-affine + opacity-multiplier.
   - Viewport identity-scale to bbox size.
   - `wl_subsurface.set_position(bbox.tx, bbox.ty)` from translation + rotation offset.
   - Acquire wl_buffer from `bake.pool`; `attach`, `damage_buffer`, `commit`, `flush`.

### 6.6 `destroy(key)`

`wl_subsurface.destroy`, `wl_surface.destroy`, drop viewport + alpha_modifier proxies, drop source_dest + bake `SurfacePair`s (their `Drop` releases dmabuf fds + Vulkan resources). Remove from `surfaces`.

### 6.7 `OsCompositorBackend` impl

```rust
fn interop_backend(&self) -> InteropBackend { InteropBackend::Vulkan }
fn sync_mechanism(&self) -> SyncMechanism { SyncMechanism::None }   // same-queue FIFO; synchronizer dormant
```

## 7. Item 2 — dmabuf import

### 7.1 `ExportableImage` allocation

Lives in `compositor_wayland/dmabuf.rs`. Per-image:

1. Pull ash handles via wgpu-hal:
   ```rust
   let hal_device = host.device.as_hal::<wgpu::wgc::api::Vulkan>().ok_or(...)?;
   let vk_device     = hal_device.raw_device().clone();
   let vk_phys       = hal_device.raw_physical_device();
   let vk_instance   = hal_device.shared_instance().raw_instance().clone();
   drop(hal_device);
   ```
2. `VkImageCreateInfo` with `format = R8G8B8A8_UNORM`, `tiling = DRM_FORMAT_MODIFIER_EXT`, `usage = TRANSFER_DST | SAMPLED | COLOR_ATTACHMENT`, chained `VkExternalMemoryImageCreateInfo { handle_types: DMA_BUF_EXT }` + `VkImageDrmFormatModifierListCreateInfoEXT { modifiers: &[chosen_modifier] }`.
3. `vkGetImageMemoryRequirements2`; `VkMemoryAllocateInfo` chained with `VkExportMemoryAllocateInfo { DMA_BUF_EXT }` + `VkMemoryDedicatedAllocateInfo` (RADV requires dedicated for exportable).
4. `vkAllocateMemory`; `vkBindImageMemory`.
5. `external_memory_fd.get_memory_fd(VkMemoryGetFdInfoKHR { handle_type: DMA_BUF_EXT })` → `OwnedFd`.
6. `vkGetImageDrmFormatModifierPropertiesEXT` to learn plane count (modal: 1 for v1's LINEAR-only choice); `vkGetImageSubresourceLayout` per plane → `PlaneLayout { offset, pitch }`.
7. Wrap into `wgpu::Texture` via `wgpu::hal::vulkan::Device::texture_from_raw` with a drop-callback that destroys the VkImage + frees VkDeviceMemory, then `Device::create_texture_from_hal::<Vulkan>`.

```rust
pub struct ExportableImage {
    vk_device: ash::Device,
    vk_image: vk::Image,
    vk_memory: vk::DeviceMemory,
    dmabuf_fd: OwnedFd,                 // CLOEXEC; dup'd when handed to wayland
    width: u32,
    height: u32,
    drm_format: u32,                    // DRM_FORMAT_ABGR8888
    drm_modifier: u64,
    planes: SmallVec<[PlaneLayout; 1]>,
    pub wgpu_texture: wgpu::Texture,
}
```

`Drop`: drop `wgpu_texture` first (it owns the in-flight cleanup callback); destroy any residual `vk_image`/`vk_memory`; close `dmabuf_fd`.

### 7.2 Modifier negotiation

v1 ships **LINEAR-only**. Infrastructure stays in place (`ModifierTable` collecting Mutter's advertised tranches; Vulkan importable set via `vkGetPhysicalDeviceImageFormatProperties2`; intersection), but the picker hard-codes `DRM_FORMAT_MOD_LINEAR` after verifying it's in the intersection. Promoting to a tile-preferred chooser later is a one-line change inside the picker.

Choice rationale: eliminates a class of "wrong tile" bugs on niche compositors at the cost of memory bandwidth that's invisible at smoke cadence.

### 7.3 `SurfaceBufferPool`

Per `ExportableImage`, N=2 slots — the mailbox pattern mainstream Wayland clients use.

```rust
pub struct SurfaceBufferPool {
    slots: [BufferSlot; 2],
    width: u32,
    height: u32,
    format_modifier: (u32, u64),
}
struct BufferSlot {
    wl_buffer: WlBuffer,
    in_flight: bool,
}
```

- **Acquire**: take first `!in_flight` slot, set `in_flight = true`, return `&WlBuffer`.
- **Starvation**: both `in_flight` → `event_queue.roundtrip()` until a release event arrives. Steady-state never blocks.
- **Release wiring**: `wayland_client::Dispatch<WlBuffer, BufferUserData>` impl marks the matching slot `in_flight = false`. `BufferUserData = (SurfaceKey, slot_index)` so the dispatcher routes correctly.

### 7.4 `wl_buffer` construction

Once per `ExportableImage` at pool init:

```rust
let params = dmabuf.create_params(&queue_handle, ());
params.add(dup_fd, plane_index=0, offset=plane.offset, stride=plane.pitch,
           modifier_hi=(modifier >> 32) as u32, modifier_lo=modifier as u32);
let wl_buffer = params.create_immed(width, height, drm_format, 0, &queue_handle, user_data);
```

`create_immed` (v3 of the protocol) avoids the async "created" event round-trip; reused at the same shape v4 advertises.

### 7.5 Errors

`BackendError::Dmabuf(String)` carrying the failing-step name + Vulkan/Wayland error code. Examples:
- `"vkAllocateMemory(modifier=0x..., size=N): ErrorOutOfDeviceMemory"`
- `"params.add(modifier=0x...): dmabuf_fd already invalid"`

## 8. Item 3 — `VulkanTimelineSemaphoreSynchronizer`

### 8.1 Type

```rust
pub struct VulkanTimelineSemaphoreSynchronizer {
    vk_device: ash::Device,
    timeline_semaphore: vk::Semaphore,
    external_semaphore_fd: ash::khr::external_semaphore_fd::Device,
    next_value: AtomicU64,
}
```

### 8.2 Inherent methods

```rust
impl VulkanTimelineSemaphoreSynchronizer {
    pub fn new(host: &HostWgpuContext) -> Result<Self, InteropError>;
    pub fn semaphore(&self) -> vk::Semaphore;
    pub fn device(&self) -> &ash::Device;
    pub fn next_value(&self) -> u64;                                          // monotonic reserve
    pub fn signaled_value(&self) -> Result<u64, InteropError>;                // vkGetSemaphoreCounterValue
    pub fn wait_host(&self, value: u64, timeout_ns: u64) -> Result<(), InteropError>;
    pub fn export_fd(&self) -> Result<OwnedFd, InteropError>;                 // OPAQUE_FD via vkGetSemaphoreFdKHR
}
```

### 8.3 Construction details

```rust
let type_info = vk::SemaphoreTypeCreateInfo::default()
    .semaphore_type(vk::SemaphoreType::TIMELINE)
    .initial_value(0);
let export_info = vk::ExportSemaphoreCreateInfo::default()
    .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
let create_info = vk::SemaphoreCreateInfo::default()
    .push_next(&mut type_info)
    .push_next(&mut export_info);
let timeline_semaphore = unsafe { vk_device.create_semaphore(&create_info, None) }?;
let external_semaphore_fd = ash::khr::external_semaphore_fd::Device::new(&vk_instance, &vk_device);
```

The export-fd hint must be baked in at creation per the Vulkan spec; semaphores not created exportable can't be `vkGetSemaphoreFdKHR`'d later.

Required Vulkan extensions: `VK_KHR_timeline_semaphore` (core in 1.2), `VK_KHR_external_semaphore_fd`. Both confirmed on RADV / Mesa 26 via earlier `vulkaninfo` pass.

### 8.4 Method semantics

- `next_value()`: `fetch_add(1) + 1` on the atomic. Pure bookkeeping — does not change the GPU-side semaphore value. Producers that integrate the handle into their own `pSignalSemaphoreValues` will move the GPU value; the atomic exists so concurrent producers can claim disjoint values without coordination.
- `signaled_value()`: `unsafe { vk_device.get_semaphore_counter_value(timeline_semaphore) }`. The live GPU view.
- `wait_host(value, timeout_ns)`: `vk_device.wait_semaphores(&SemaphoreWaitInfo { flags: WAIT_ALL, semaphores: [timeline_semaphore], values: [value] }, timeout_ns)`. Blocks the calling thread.
- `export_fd()`: `unsafe { external_semaphore_fd.get_semaphore_fd(&SemaphoreGetFdInfoKHR { semaphore, handle_type: OPAQUE_FD }) }`, wrap in `OwnedFd`. Caller owns it; each call returns a fresh duplicate.

### 8.5 Drop

```rust
impl Drop for VulkanTimelineSemaphoreSynchronizer {
    fn drop(&mut self) {
        unsafe { self.vk_device.destroy_semaphore(self.timeline_semaphore, None) };
    }
}
```

### 8.6 Backend wiring

`WaylandSubsurfaceBackend` holds the synchronizer as a field, constructed in `new`. **Not** advanced in `present_master` — the smoke path uses implicit dmabuf sync; the synchronizer is dormant. `signaled_value()` reads 0 throughout the smoke. Its presence is what fills the lineage slot; idiomatic Vulkan use happens when a real consumer wires the handle into its own submits.

### 8.7 Tests

`#[cfg(test)] mod tests` in `vulkan_timeline.rs`:
- `new` returns `InteropError::BackendMismatch` when handed a non-Vulkan `HostWgpuContext`.
- `next_value` is monotonic across threads.

GPU semantics (signal-via-real-submit, wait_host on signaled value, fd export round-trip) get exercised by the smoke in Section 9 if we extend it; for v1 they're covered structurally by `new` succeeding on the real device.

## 9. Item 4 — Pelt smoke

### 9.1 Files

- New `ports/pelt-desktop/smoke_wayland.rs`.
- `ports/pelt-desktop/lib.rs` adds `#[cfg(feature = "linux-present")] pub mod smoke_wayland;` + re-exports `WaylandPresentSmokeConfig` / `WaylandPresentSmokeOutcome` / `run_wayland_subsurface_present_smoke`.
- `ports/pelt-desktop/Cargo.toml` adds the `linux-present` feature.
- `ports/pelt/Cargo.toml` adds `linux-present = ["pelt-desktop/linux-present"]`.
- `ports/pelt/viewer.rs` adds the two flags.

### 9.2 Config + outcome

```rust
#[cfg(feature = "linux-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,                      // 0 = run until window close (visual receipt)
    pub declare_subsurface: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub frames_presented: u32,
    pub created_window: bool,
    pub declared_subsurface: bool,
}
```

### 9.3 Runtime path

`run_wayland_subsurface_present_smoke(config) -> Result<WaylandPresentSmokeOutcome, String>`:

- `cfg(not(target_os = "linux"))` short-circuit: `Err("linux-present requires target_os = \"linux\"".into())`.
- `cfg(target_os = "linux")`: build a `winit::event_loop::EventLoop`, `WaylandPresentApp::new(config)`, `run_app`. Surface app errors / outcome.

`WaylandPresentApp` (`winit::application::ApplicationHandler`):

- `resumed`:
  1. Create winit window from `WindowAttributes`.
  2. `HasDisplayHandle::display_handle` + `HasWindowHandle::window_handle` → raw handles.
  3. `build_vulkan_handles()` (below) → `netrender::WgpuHandles`.
  4. `netrender::create_netrender_instance(handles, NetrenderOptions { tile_cache_size: Some(64), enable_vello: true, .. })`.
  5. `paint::HostWgpuContext::new(device, queue)`.
  6. `paint::default_compositor_for_window(host, display_handle, window_handle)` → `Box<dyn PaintCompositor>` wrapping `ServoCompositor<WaylandSubsurfaceBackend>`.
  7. Stash `WaylandPresentState { renderer, compositor }` into `self.state = Some(...)`. `window.request_redraw()`.

- `window_event(RedrawRequested)`:
  1. `frames > 0 && frames_presented >= frames` → return (defensive guard, mirrors macOS).
  2. Pull `window.inner_size()` → `backing_w × backing_h`.
  3. Build `netrender::Scene::new(backing_w, backing_h)` + push full-viewport red rect.
  4. If `config.declare_subsurface`: push green top-left quarter rect; declare `CompositorSurface::new(SurfaceKey(1), [0, 0, half_w, half_h])` with `opacity = 0.5`.
  5. `renderer.render_with_compositor(&scene, Rgba8Unorm, &mut **state.compositor, Color::TRANSPARENT)`.
  6. `frames_presented += 1`.
  7. `frames > 0 && frames_presented >= frames` → `event_loop.exit()`; else `window.request_redraw()`.

- `exiting`: finalize `WaylandPresentSmokeOutcome`.

```rust
fn build_vulkan_handles() -> Result<netrender::WgpuHandles, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|err| format!("request_adapter: {err}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pelt wayland-present device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits {
            max_inter_stage_shader_variables: 28,
            ..Default::default()
        },
        ..Default::default()
    }))
    .map_err(|err| format!("request_device: {err}"))?;
    Ok(netrender::WgpuHandles { instance, adapter, device, queue })
}
```

### 9.4 Viewer flags

```rust
#[cfg(feature = "linux-present")]
let mut wayland_present_smoke = false;
#[cfg(feature = "linux-present")]
let mut wayland_present_surfaces_smoke = false;

#[cfg(feature = "linux-present")]
"--wayland-present-smoke" => { wayland_present_smoke = true; }
#[cfg(feature = "linux-present")]
"--wayland-present-surfaces-smoke" => { wayland_present_surfaces_smoke = true; }
```

Plus dispatch + help text:
```
--wayland-present-smoke             (requires --features linux-present, target_os = "linux")
--wayland-present-surfaces-smoke    (same as --wayland-present-smoke + a declared compositor surface)
```

### 9.5 Smoke defaults

- Basic smoke `WaylandPresentSmokeConfig::default`: `frames = 60` (path validation; ~1s at 60Hz).
- Surfaces smoke: `frames = 0` (hold until window close — visual receipt).

## 10. Validation & done condition

### 10.1 Per-item gates

- After item 3 (synchronizer): `cargo check -p servo-paint` succeeds.
- After items 1+2 (backend + dmabuf): `cargo check -p servo-paint` succeeds.
- After item 4 (smoke): `cargo build -p pelt --features linux-present` succeeds on the Fedora 44 box.
- Throughout: `cargo clippy -p servo-paint -- -D warnings` clean (paint already denies `unwrap_used` / `panic` at the crate root).

### 10.2 Done-condition receipts

Two smokes, mirroring the macOS/Windows convention (basic = auto-exit headless validation, surfaces = held-window visual receipt):

1. **Headless basic** (auto-exits after `frames` redraws — default 60): `pelt --wayland-present-smoke` → exit 0, stdout `pelt wayland-present smoke {W}x{H} frames=60 created_window=true declared_subsurface=false`. Validates the master path end-to-end (dmabuf export + parent-surface attach + commit + flush) without user interaction.
2. **Visual receipt + headless surfaces validation** (held window; user closes): `pelt --wayland-present-surfaces-smoke` → window stays open until close; on close, exit 0, stdout `pelt wayland-present surfaces smoke {W}x{H} frames={N} created_window=true declared_subsurface=true` (N being whatever frames painted before close). The visual content **is** the per-`SurfaceKey` validation: red master fills viewport, green top-left quarter at 50% opacity producing olive/yellow blend where the per-surface composes over master red, pure green where master green is occluded. Closing exit-clean confirms `declare`→`present`→`destroy` runs without resource leaks.

### 10.3 Per-frame visibility check

`WAYLAND_DEBUG=1 pelt --wayland-present-surfaces-smoke 2>&1 | head -60` should show:
- `wl_compositor.create_surface` + `wl_subcompositor.get_subsurface` + `wp_viewporter.get_viewport` + (optional) `wp_alpha_modifier_v1.get_surface` at declare.
- `zwp_linux_dmabuf_v1.create_params` + `params.add` + `params.create_immed` once per pool slot.
- Per frame: `wl_surface.attach(buffer, 0, 0)` + `damage_buffer(0, 0, w, h)` + `commit` for master and each per-key.
- `wl_buffer.release` events arriving steadily from Mutter.

A first-run modifier log message: `[WaylandSubsurfaceBackend] dmabuf modifier: DRM_FORMAT_ABGR8888 / 0x0000000000000000 (LINEAR)`.

## 11. Doc updates at landing

- `docs/2026-05-09_c4_landed_notes.md` — flip gap (4) to ✅ with the smoke commands + visual receipt; mirror the style used for items 0/1/2.
- `docs/2026-05-09_interop_lineage.md` — flip the Linux slot from "Pending" to ✓; brief note on shape (`VulkanTimelineSemaphoreSynchronizer` exposing the semaphore handle + `next_value` + `signaled_value` via `vkGetSemaphoreCounterValue` + `wait_host` via `vkWaitSemaphores` + OPAQUE_FD export).
- `docs/2026-05-28_wayland_per_surface_presentation_gap.md` — append "Done 2026-06-03" annotation linking the smoke commands + receipt.
- `docs/archive/2026-05-05_genet_netrender_cut_plan.md` — Linux status snapshot moves from pending to landed; C4 universally ✅.

## 12. Risks & open questions

- **Modifier negotiation surfaces empty** — first-run `log::info!` at backend `new` reveals the chosen `(format, modifier)`. If LINEAR isn't in Mutter's advertised tranches on Fedora 44 (extremely unlikely; LINEAR is baseline), the picker can pivot to whatever tile modifier is advertised; modifier negotiation infrastructure is already in place.
- **`wp_alpha_modifier_v1` advertisement absent on Mutter 47** — the bake-path opacity fallback is the safety net. The visual receipt still shows the correct 50% blend either way; only the GPU cost differs (one extra render pass when baked).
- **Buffer pool starvation at startup** — both slots are `!in_flight` at construction, no roundtrip needed for the first two frames; `event_queue.roundtrip()` covers the pathological "Mutter holding both" case.
- **winit + parent-surface buffer attach interplay** — winit handles `xdg_surface.configure` ack internally; the smoke owns parent-surface attaches/commits. Mainline pattern; will know on first run.
- **First Vulkan dmabuf export ever** — graft's `sync_vulkan.rs` is the structural reference for the ash glue. The largest unknown is the RADV-specific behavior of `VkExportMemoryAllocateInfo` + dedicated allocation on `DRM_FORMAT_MOD_LINEAR`; Mesa 26's docs say it's supported; first run confirms.

## 13. Implementation order

Suggested sequencing (each commit lands cleanly):

1. **`vulkan_timeline.rs` + interop wiring** — smallest dep surface; ash-only; unit tests cover construction and counter monotonicity. Confirms ash wires into the wgpu-hal Vulkan device cleanly.
2. **`compositor_wayland/dmabuf.rs`** — `ExportableImage` + `SurfaceBufferPool` + modifier negotiation, behind a small test that constructs a single `ExportableImage` from a Vulkan-forced wgpu device and verifies `wgpu_texture.size()` matches.
3. **`compositor_wayland/wayland.rs` + `mod.rs`** — wayland-client connection, registry, globals, per-`SurfaceKey` state; `present_master` + `declare` + `present` + `destroy` wired. Backend constructible end-to-end; integration with `default_compositor_for_window` exercised via the existing factory.
4. **`compositor_wayland/bake.rs`** — bake pipeline (vertex + fragment shader, render pass).
5. **`smoke_wayland.rs` + viewer flags** — smoke runner. Headless gate green.
6. **Visual receipt** — run with `frames = 0`, eyeball.
7. **Docs** — flip gap (4), interop lineage Linux slot, brief, cut plan.

Each step is independently `cargo check`-able and behind a clean boundary; if step 3 turns up a surprise, steps 1-2 are still mergeable as preparatory infra.
