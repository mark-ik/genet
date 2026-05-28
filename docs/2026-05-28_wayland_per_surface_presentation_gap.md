# Wayland per-surface presentation — the last C4 platform gap

The netrender compositor handoff (C4 / D3.5b) presents through the
native OS compositor on two of three desktop targets. Windows
(DXGI Composition / DCOMP child-visuals) was verified on hardware
2026-05-25; macOS (CALayer / IOSurface) was verified 2026-05-09.
Linux Wayland is the remaining platform. This note is the handoff
brief for finishing it on a live session.

Companion docs:

- [2026-05-09_c4_landed_notes.md](./2026-05-09_c4_landed_notes.md) — what landed on C4; gap (4) is this work
- [2026-05-09_interop_lineage.md](./2026-05-09_interop_lineage.md) — the direction-neutral interop primitives the backend consumes; the Linux synchronizer slot
- [archive/2026-05-05_compositor_handoff_path_b_prime.md](./archive/2026-05-05_compositor_handoff_path_b_prime.md) — the path-(b′) design

---

## Current state

`WaylandSubsurfaceBackend` in
[components/paint/compositor_wayland.rs](../components/paint/compositor_wayland.rs)
is a compiling skeleton. The constructor accepts `wl_display` +
`wl_surface` raw pointers and allocates a per-`SurfaceKey`
`wl_subsurface`. The per-frame body is declared but never run, so
nothing has presented through a Wayland compositor yet.

The surrounding plumbing is platform-agnostic and already landed:

- [compositor.rs](../components/paint/compositor.rs) — `OsCompositorBackend` trait and `ServoCompositor<B>::present_frame`, which iterates `frame.layers`, allocates per-`SurfaceKey` destination textures, copies master→layer, and calls `backend.present(key, transform, clip, opacity)`. Wayland overrides nothing here yet.
- [compositor_factory.rs](../components/paint/compositor_factory.rs) — `default_compositor_for_window` cfg-dispatches to `WaylandSubsurfaceBackend` on Linux, falling back to `WgpuMasterCaptureBackend` offscreen.
- [interop/](../components/paint/interop/) — `HostWgpuContext` and the inherent-method synchronizer pattern. Only `Dx12FenceSynchronizer` ships today.

## What's left

1. **Per-frame subsurface body.** Implement `wl_subsurface` placement + `wl_surface.commit` in the backend's per-frame path so each declared `SurfaceKey` lands as a subsurface of the embedder `wl_surface`, with transform / clip / opacity applied.
2. **dmabuf import.** Bridge the netrender wgpu texture to a `wl_buffer` over the linux-dmabuf protocol. The producer is netrender's own wgpu queue, reached through wgpu-hal's Vulkan `as_hal` accessors (same shape as the Windows and macOS backends; no GL path — serval is wgpu-only post-C1).
3. **`VulkanTimelineSemaphoreSynchronizer`.** Add the Linux slot to `crate::interop` as direction-neutral inherent methods, driven by the backend the way `WindowsDxgiBackend` drives `Dx12FenceSynchronizer`. Vulkan timeline semaphores are the canonical cross-API fence on Linux; graft's `sync_vulkan.rs` is the structural reference.
4. **Pelt smoke mode.** Add `--wayland-present-surfaces-smoke` to [ports/pelt/viewer.rs](../ports/pelt/viewer.rs), mirroring `--windows-present-surfaces-smoke` / `--macos-present-surfaces-smoke`. Run under Mutter or Sway.

## Done condition

A live smoke receipt on Fedora Wayland: master present and
per-`SurfaceKey` subsurface present both run clean, exit 0, with the
declared-subsurface flag set. Then a manual visual-color receipt
(red master, green declared surface at 50% opacity, olive where they
blend), the same receipt Windows and macOS produced. Until that runs
on hardware, C4 stays externally gated on Linux and D3 is not yet ✅
there.

## Target hardware

Fedora 44 workstation (Wayland) is the validation box. The Mint Acer
is X11, so it does not exercise this path.
