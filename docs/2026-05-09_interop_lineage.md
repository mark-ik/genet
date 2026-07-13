# Interop primitives — lineage

How [`components/paint/interop/`](../components/paint/interop/)
got its current shape across four iterations, and why the
direction-neutral pieces are stable.

## Four iterations

1. **slint example** (upstream). System-webview → Slint render
   interop using raw GL. Producer = system webview (`WKWebView` /
   `IWebView2` / equivalent), consumer = Slint scene render. GL
   textures, EGL/CGL/WGL plumbing, hand-rolled per-platform
   handles. Each platform was its own bespoke surface; no shared
   abstraction.

2. **`wgpu-graft`** ([github.com/mark-ik/wgpu-graft](https://github.com/mark-ik/wgpu-graft)).
   Moved off raw GL onto wgpu-native texture interop. First
   shaping pass for the per-platform sync into a uniform
   `InteropSynchronizer` trait — Vulkan timeline semaphore,
   DX12 shared fence, Metal shared event — paired with
   `NativeFrame` (an enum of producer-side platform handles)
   and `ImportedTexture` (the consumer-side wgpu shape) carrying
   producer/consumer handles through the trait. Still **import
   direction**: producer is some external system surface,
   consumer is wgpu.

3. **`scrying`**. Mac-focused iteration replacing the webview
   producer with ScreenCaptureKit (SCKit). Same import shape,
   different producer. Sharpened the per-platform synchronizer
   wrappers (especially the Metal `MTLSharedEvent` path) and
   surfaced `HostWgpuContext`, `InteropBackend`, `SyncMechanism`
   as direction-neutral building blocks usable across producers.
   The trait shape stayed import-coupled.

4. **genet** (`components/paint/interop/`). **Direction
   reversed.** Producer is now netrender (wgpu's hidden
   Vulkan / Metal / DX12 queue); consumer is the OS compositor
   (DCOMP / CALayer / Wayland subsurface). Import-shaped types
   (`NativeFrame`, `ImportedTexture`, `InteropSynchronizer`)
   don't fit — they encode an import flow that doesn't exist
   when genet *is* the producer. The direction-neutral pieces
   survived intact and moved here.

## What carried over (the "bit in between")

| Type / function | Role |
| --- | --- |
| `InteropBackend` | Discriminator (Vulkan / Metal / Dx12 / Unknown) |
| `HostWgpuContext` | `device + queue + backend` bundle |
| `SyncMechanism` | Producer→consumer fence kind (None / ExplicitFence / ExplicitExternalSemaphore) |
| `Dx12FenceSynchronizer` | DX12 shared-fence wrapper |
| `detect_backend(&Device)` | Backend detection via `as_hal::<A>()` probes |

These don't care which side of the producer/consumer arrow
you're on. They identify the GPU backend, hold the device/queue,
name the sync mechanism, and wrap each platform's shared-fence
object — all useful regardless of who is producing what.

All in [components/paint/interop/](../components/paint/interop/).

## What genet drops, and why — the trait-shape mismatch concretely

The earlier iterations defined synchronization through a trait
roughly shaped like (illustrative-signature-only):

```rust
trait InteropSynchronizer {
    fn producer_complete(
        &self,
        frame: &NativeFrame,        // import-side: producer's handles
        mechanism: SyncMechanism,
    ) -> Result<(), InteropError>;

    fn consumer_ready(
        &self,
        texture: &ImportedTexture,  // import-side: wgpu wrapper
        mechanism: SyncMechanism,
    ) -> Result<(), InteropError>;
}
```

Both methods take parameters that **only exist in the import
direction**:

- `&NativeFrame` is an enum like `{ MetalTextureRef, Dx12SharedTexture,
  VulkanDmabuf, … }` — producer-side platform handles being handed
  *into* the consumer's wgpu world. In genet, the producer is
  *netrender's own wgpu queue*; there is no native frame from an
  external producer to package up.
- `&ImportedTexture` is the wgpu-side wrapper post-import (carrying
  format, size, generation, the consumer-sync handle). genet
  doesn't import a texture — it allocates its own destination
  textures via `OsCompositorBackend::declare` and renders into
  them.

So the trait signatures don't translate. Two ways to deal with
that, and genet picked the second:

1. **Patch `InteropSynchronizer`** — make `NativeFrame` /
   `ImportedTexture` into `Option`s, or split the trait into
   producer / consumer halves. Mutates the upstream surface.
2. **Drop the trait, keep the inherent methods.** Each
   per-platform synchronizer (e.g. `Dx12FenceSynchronizer`)
   exposes inherent methods (`advance`, `current_value`,
   `queue_wait`) that the platform's `OsCompositorBackend` impl
   calls into directly. The backend impl owns the per-frame
   fence dance — there is no shared trait on the synchronizer
   side.

Picking (2) means the trait shape can stay import-coupled in
graft/scrying without genet pulling on it, and genet's per-
platform fence usage is per-`OsCompositorBackend`-impl rather
than indirected through a generic trait. **Per project policy:
genet does not shape graft / WNTI / scrying to fit its needs.**
The price is that genet's
[`crate::interop`](../components/paint/interop/) inlines the
direction-neutral primitives instead of importing them as a
crate.

## What `crate::interop` doesn't carry

- **No `InteropSynchronizer` trait.** Each platform's
  [`OsCompositorBackend`](../components/paint/compositor.rs) impl
  owns the per-frame fence dance directly via inherent-method
  calls on its synchronizer. See above.
- **No `NativeFrame` / `ImportedTexture` / `WgpuTextureImporter`
  types.** Producer-side handles don't exist in genet; the
  producer is netrender's own wgpu queue, accessed via wgpu-hal's
  `as_hal::<A>().raw_*()` accessors.
- **No GL plumbing** (`vulkan_dmabuf`, `raw_gl`, `surfman_gl`,
  GL `ProducerCapabilities`). genet's renderer is wgpu-only
  post-C1 (the GL/surfman corpus cut).

## Pending: Mac synchronizer wrapper

Currently [`Dx12FenceSynchronizer`](../components/paint/interop/windows_dx12.rs) and
[`VulkanTimelineSemaphoreSynchronizer`](../components/paint/interop/vulkan_timeline.rs)
ship in `crate::interop`. The macOS slot is the only inferred one remaining:

- **macOS — `MTLSharedEventSynchronizer` (or similar).** Land alongside the
  GPU-side wait path on Mac. Today
  [`MacosCALayerBackend`](../components/paint/compositor_calayer/mod.rs)
  holds a raw `Retained<ProtocolObject<dyn MTLSharedEvent>>` field and
  CPU-stalls via `wgpu::Device::poll(Wait)`; lifting that into a typed
  synchronizer wrapper is the natural follow-up once `wgpu-hal::metal::Queue`
  exposes its underlying `MTLCommandQueue` (so the producer can
  `encodeSignalEvent` on the same queue netrender submits to). scrying's
  `sync_metal.rs` is the structural reference.

The Linux slot is filled as of 2026-06-03:

- **Linux Wayland — `VulkanTimelineSemaphoreSynchronizer`** (landed 2026-06-03,
  [`components/paint/interop/vulkan_timeline.rs`](../components/paint/interop/vulkan_timeline.rs)).
  Idiomatic Vulkan-timeline shape: the semaphore handle is the API
  (`semaphore()` returns `vk::Semaphore`); producers wire it into
  their own `VkSubmitInfo.pSignalSemaphores`/`pSignalSemaphoreValues`,
  consumers into `pWaitSemaphores`. `next_value()` reserves the next
  monotonic value; `signaled_value()` reads the GPU-side counter via
  `vkGetSemaphoreCounterValue`; `wait_host(value, timeout_ns)` blocks
  the calling thread via `vkWaitSemaphores`; `export_fd()` exports an
  OPAQUE_FD via `vkGetSemaphoreFdKHR` for cross-process / external-
  driver consumers. No empty-buffer signal/wait submits — those are
  not how timeline semaphores get used in real Vulkan code. The slot
  is dormant on the C4 smoke path (`WaylandSubsurfaceBackend` returns
  `SyncMechanism::None`; same-queue FIFO covers the per-frame model),
  but the wrapper is constructed and verifiable via `signaled_value()
  → Ok(0)` at backend construction time.

Both wrappers stay direction-neutral (inherent methods, no trait); the
per-platform `OsCompositorBackend` impl drives them the same way
`WindowsDxgiBackend` drives the Dx12 one today.

## Recipe for a new platform backend

When wiring (or extending) a per-platform `OsCompositorBackend`,
reach for `crate::interop` for:

1. **The wgpu↔native handle bridge.** `HostWgpuContext::new(device,
   queue)` auto-detects the backend; the resulting bundle is what
   `OsCompositorBackend::declare` receives. Don't construct your
   own — the detection pass keeps `host.backend ==
   backend.interop_backend()` checked at `ServoCompositor::new`
   time.
2. **The shared-fence wrapper for your platform** if one exists.
   DX12 and Linux Vulkan-timeline are both in `crate::interop`; Mac is
   pending (above). For now, the Mac path holds an `MTLSharedEvent`
   directly in the backend struct; promote to a `crate::interop` wrapper
   when GPU-side wait lands.
3. **The `SyncMechanism` discriminator** — return the right variant
   from `OsCompositorBackend::sync_mechanism()` so consumers of the
   trait can branch on it.

Don't reach for WNTI — it's not in the genet dep graph (was
removed in [commit `d0dea13` "cargo dependency fixes"](../components/paint/interop/mod.rs))
and the trait shape is wrong for the export direction anyway.

## When to revise this brief

When `crate::interop` grows beyond what carried over from
graft/scrying — e.g., the GPU-side `MTLSharedEvent` synchronizer
wrapper lands on Mac (the Vulkan timeline-semaphore wrapper already
landed on Linux in 2026-06-03), an IOSurface allocation helper joins
the module. At that point, this brief becomes the history;
document the new shape in this file's "What's here" section
and let this lineage section stand as provenance.
