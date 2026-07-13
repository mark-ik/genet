# Wayland per-surface presentation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the `WaylandSubsurfaceBackend` end-to-end so `pelt --wayland-present-surfaces-smoke` exits 0 with `declared_subsurface=true` and the visual color receipt (red master, green declared-quarter at 50% opacity, olive blend) renders correctly on Fedora 44 + GNOME/Mutter — clearing C4/D3 gap (4) on Linux.

**Architecture:** Per-`SurfaceKey` `wl_subsurface` over the embedder's parent `wl_surface`; per-key destination textures allocated as dmabuf-exportable VkImages (via `ash` + `VK_EXT_image_drm_format_modifier`) and wrapped back into `wgpu::Texture` via `create_texture_from_hal::<Vulkan>`; implicit sync via `wl_buffer.release` events; `wp_viewporter` for transform/clip and `wp_alpha_modifier_v1` for opacity with a wgpu-render-pass bake fallback that also handles rotation. Idiomatic-Vulkan `VulkanTimelineSemaphoreSynchronizer` fills the lineage-doc Linux interop slot, dormant on the smoke path.

**Tech Stack:** Rust 2024 edition, `wgpu = "29"` (vulkan feature), `wgpu-hal::vulkan` (`raw_device`, `raw_instance`, `as_raw`, `texture_from_raw`, `create_texture_from_hal::<Vulkan>`), `ash = "0.38"`, `wayland-client = "0.31"`, `wayland-protocols = "0.32"` (linux-dmabuf-v1 v3/v4, viewporter, alpha-modifier-v1), `winit = "0.30"` (driving the smoke window), Fedora 44 + RADV (Mesa 26.0.6) + GNOME/Mutter.

**Companion spec:** [`docs/2026-06-03_wayland_per_surface_presentation_design.md`](./2026-06-03_wayland_per_surface_presentation_design.md). Each task below cites the spec section it implements.

**Note on TDD scaling:** Pure-logic and FFI-error-path code (synchronizer construction validation, atomic counters, modifier negotiation predicate) gets first-class unit tests. Vulkan/Wayland integration code (`ExportableImage` allocation, `present_master` per-frame body) is integration-tested through the smoke runner — the smoke is the test. Where a task has no isolated test target, the verification step is `cargo check -p servo-paint` (compile gate) followed by the next phase's integration check.

---

## Phase 1 — Cargo dependency wiring & feature plumbing

Lays the dep + feature surface so subsequent commits compile cleanly. No runtime code yet. Spec §5.

### Task 1.1: Linux-target deps in `servo-paint` + `linux-present` feature in `pelt-desktop` / `pelt`

**Files:**
- Modify: `components/paint/Cargo.toml`
- Modify: `ports/pelt-desktop/Cargo.toml`
- Modify: `ports/pelt/Cargo.toml`

- [ ] **Step 1: Add the Linux-target dependency block in `components/paint/Cargo.toml`.**

Append (preserving the existing target.cfg block ordering):

```toml
[target.'cfg(target_os = "linux")'.dependencies]
# C4 Wayland compositor backend (compositor_wayland/). ash version matches
# wgpu-hal 29's lockfile-pinned 0.38.0+1.3.281 so the types line up.
ash = "0.38"
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["client", "unstable", "staging"] }
```

- [ ] **Step 2: Add the `linux-present` feature in `ports/pelt-desktop/Cargo.toml`.**

Insert under `[features]` after the `macos-present` block (preserving alphabetical/declared order):

```toml
# Headed presentation through genet's `WaylandSubsurfaceBackend`. Drives
# the netrender renderer through a winit Wayland window with per-`SurfaceKey`
# subsurfaces. Linux-only at runtime; the feature gate is platform-agnostic so
# `cargo check --features linux-present` works on non-Linux hosts (the inner
# code is `cfg(target_os = "linux")` gated). The wl_display + wl_surface
# extraction lives in `paint::compositor_factory`; pelt-desktop calls
# `default_compositor_for_window` and doesn't need its own wayland-client deps.
linux-present = [
    "netrender",
    "dep:paint",
    "dep:raw-window-handle",
    "dep:pollster",
]
```

- [ ] **Step 3: Add the `linux-present` pass-through feature in `ports/pelt/Cargo.toml`.**

Find the `[features]` block (mirrors the windows-present / macos-present pass-throughs) and add:

```toml
linux-present = ["pelt-desktop/linux-present"]
```

- [ ] **Step 4: Verify the workspace resolves.**

Run: `cargo check -p servo-paint`
Expected: success (no new code references the new deps yet, so this just confirms the manifest parses and resolves).

Run: `cargo check -p pelt --features linux-present`
Expected: success.

- [ ] **Step 5: Confirm gating discipline holds for non-Linux targets.**

Run: `cargo tree -p pelt --target x86_64-pc-windows-msvc 2>&1 | grep -E '^(\| )*(ash|wayland-client|wayland-protocols)' || echo OK_no_linux_deps_on_windows`
Expected: `OK_no_linux_deps_on_windows` (the Linux target block is invisible to Windows resolution).

- [ ] **Step 6: Commit.**

```bash
git add components/paint/Cargo.toml ports/pelt-desktop/Cargo.toml ports/pelt/Cargo.toml
git commit -m "$(cat <<'EOF'
paint+pelt: Linux-target deps + linux-present feature plumbing

Adds ash 0.38, wayland-client 0.31, wayland-protocols 0.32 under
[target.'cfg(target_os = "linux")'.dependencies] in servo-paint, and
the linux-present feature in pelt-desktop with a matching pass-through
in pelt. No code yet; just the dep surface for C4 Wayland landing.

Mirrors the windows-present / macos-present pattern: feature gate is
platform-agnostic so `cargo check --features linux-present` works on
any host, inner code remains cfg(target_os = "linux") gated. Workspace
deps unchanged — keep ash + wayland-* local to paint until a second
consumer materialises.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `VulkanTimelineSemaphoreSynchronizer`

The Linux interop slot. Idiomatic Vulkan-timeline shape (handle + counter + host-readable signaled value + host-wait + OPAQUE_FD export); not a Dx12 look-alike. Spec §4.5 + §8.

### Task 2.1: Scaffold `vulkan_timeline.rs` with new() + unit test for BackendMismatch

**Files:**
- Create: `components/paint/interop/vulkan_timeline.rs`
- Modify: `components/paint/interop/mod.rs`

- [ ] **Step 1: Write the failing test.**

Create `components/paint/interop/vulkan_timeline.rs`:

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Linux Vulkan timeline-semaphore synchronizer.
//!
//! Direction-neutral inherent-method surface (no `InteropSynchronizer`
//! trait — see [`docs/2026-05-09_interop_lineage.md`](../../../docs/2026-05-09_interop_lineage.md)).
//! Idiomatic Vulkan-timeline shape: the semaphore handle is the API;
//! producers wire it into their own `pSignalSemaphores`/`pSignalSemaphoreValues`,
//! consumers into `pWaitSemaphores`/`pWaitSemaphoreValues`. The wrapper
//! tracks a monotonic `next_value` for value reservation, exposes the
//! host-readable signaled value via `vkGetSemaphoreCounterValue`, a
//! host-side `vkWaitSemaphores` wait, and an OPAQUE_FD export for
//! cross-process / external-driver consumers.
//!
//! Required Vulkan extensions: `VK_KHR_timeline_semaphore` (core in 1.2),
//! `VK_KHR_external_semaphore_fd`. Both ship in RADV / Mesa 26.

#![allow(unsafe_op_in_unsafe_fn)]

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;

use super::{HostWgpuContext, InteropBackend, InteropError};

/// Vulkan timeline-semaphore synchronizer. Construct one per
/// [`HostWgpuContext`] (i.e. per wgpu Vulkan device). Reuse across
/// frames. Producers wire [`semaphore`](Self::semaphore) into their own
/// `vkQueueSubmit` calls; the wrapper does not issue empty-buffer signal
/// or wait submits.
pub struct VulkanTimelineSemaphoreSynchronizer {
    vk_device: ash::Device,
    timeline_semaphore: vk::Semaphore,
    external_semaphore_fd: ash::khr::external_semaphore_fd::Device,
    next_value: AtomicU64,
}

unsafe impl Send for VulkanTimelineSemaphoreSynchronizer {}
unsafe impl Sync for VulkanTimelineSemaphoreSynchronizer {}

impl VulkanTimelineSemaphoreSynchronizer {
    /// Construct a synchronizer bound to the host's wgpu Vulkan device.
    /// Returns [`InteropError::BackendMismatch`] if `host.backend` is
    /// not [`InteropBackend::Vulkan`].
    pub fn new(host: &HostWgpuContext) -> Result<Self, InteropError> {
        if host.backend != InteropBackend::Vulkan {
            return Err(InteropError::BackendMismatch {
                expected: "Vulkan",
                actual: "non-Vulkan",
            });
        }
        // Real impl lands in 2.2.
        unimplemented!("VulkanTimelineSemaphoreSynchronizer::new — real impl in Task 2.2")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub HostWgpuContext for the BackendMismatch path. The real
    /// constructor never runs (we error before any device touch), so
    /// constructing a HostWgpuContext with a non-Vulkan backend
    /// discriminator is sufficient to drive this test.
    ///
    /// Vulkan-backed HostWgpuContext construction requires a real
    /// wgpu device, exercised in the smoke (Phase 8).
    #[test]
    fn new_returns_backend_mismatch_on_non_vulkan_host() {
        let (device, queue) = pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                })
                .await
                .expect("fallback adapter");
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    ..Default::default()
                })
                .await
                .expect("device")
        });

        // Force backend != Vulkan regardless of what detection picked,
        // so the construction validation predicate is what's exercised.
        let mut host = HostWgpuContext::new(device, queue);
        host.backend = InteropBackend::Dx12;

        let result = VulkanTimelineSemaphoreSynchronizer::new(&host);
        assert!(matches!(
            result,
            Err(InteropError::BackendMismatch { expected: "Vulkan", .. })
        ));
    }
}
```

Modify `components/paint/interop/mod.rs` — append after the existing `#[cfg(target_os = "windows")] pub use windows_dx12::Dx12FenceSynchronizer;`:

```rust
#[cfg(target_os = "linux")]
mod vulkan_timeline;

#[cfg(target_os = "linux")]
pub use vulkan_timeline::VulkanTimelineSemaphoreSynchronizer;
```

Also remove (or update) the "Pending: Mac and Linux synchronizer wrappers" comment block at the bottom — leave only the macOS pending note:

```rust
// macOS counterpart will land alongside its respective
// `OsCompositorBackend` impl. No trait shape here — backends call
// into per-platform synchronizers via inherent methods, so the
// import-direction-coupled `InteropSynchronizer` trait the upstream
// iterations carried doesn't apply. See the lineage brief at
// `docs/2026-05-09_interop_lineage.md` for the full reasoning.
```

- [ ] **Step 2: Run the test, verify it fails.**

Run: `cargo test -p servo-paint --lib interop::vulkan_timeline::tests::new_returns_backend_mismatch_on_non_vulkan_host -- --nocapture`
Expected: PASS — the BackendMismatch error path is reachable through the predicate alone; `unimplemented!()` is past the early return so it never executes on this case.

Wait — actually the test will fail to compile until `interop::HostWgpuContext.backend` is exposed as a `pub` field (it already is per `interop/mod.rs:96-98`). Re-check the compile.

Run: `cargo check -p servo-paint --tests`
Expected: success.

Run: `cargo test -p servo-paint --lib interop::vulkan_timeline -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Commit.**

```bash
git add components/paint/interop/vulkan_timeline.rs components/paint/interop/mod.rs
git commit -m "$(cat <<'EOF'
paint: scaffold VulkanTimelineSemaphoreSynchronizer + BackendMismatch test

Adds the Linux synchronizer slot named in interop_lineage.md. Only the
BackendMismatch validation lands in this commit — the Vulkan handle
allocation lands in the next.

Idiomatic Vulkan-timeline surface: the semaphore handle is the API,
producers integrate it into their own pSignal*; the wrapper exposes
`semaphore()`, `next_value()` (atomic reserve), `signaled_value()`
(vkGetSemaphoreCounterValue), `wait_host()` (vkWaitSemaphores), and
`export_fd()` (OPAQUE_FD via vkGetSemaphoreFdKHR). No empty-buffer
queue submits — those aren't how timeline semaphores get used.

Wired into interop/mod.rs behind cfg(target_os = "linux"); the
"Pending" Linux slot in the lineage doc note block is reduced to just
the macOS pending entry.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.2: Implement `new()` — timeline semaphore + export-fd loader

**Files:**
- Modify: `components/paint/interop/vulkan_timeline.rs`

- [ ] **Step 1: Replace the `unimplemented!()` in `new()` with the real construction.**

```rust
    pub fn new(host: &HostWgpuContext) -> Result<Self, InteropError> {
        if host.backend != InteropBackend::Vulkan {
            return Err(InteropError::BackendMismatch {
                expected: "Vulkan",
                actual: "non-Vulkan",
            });
        }

        let (vk_device, external_semaphore_fd, timeline_semaphore) = unsafe {
            let hal_device = host.device.as_hal::<wgpu::wgc::api::Vulkan>().ok_or(
                InteropError::BackendMismatch {
                    expected: "Vulkan",
                    actual: "non-Vulkan",
                },
            )?;
            let vk_device = hal_device.raw_device().clone();
            let vk_instance = hal_device.shared_instance().raw_instance().clone();
            drop(hal_device);

            // The export-fd hint must be baked in at creation per the
            // Vulkan spec — semaphores not created exportable cannot be
            // exported via vkGetSemaphoreFdKHR later.
            let mut type_info = vk::SemaphoreTypeCreateInfo::default()
                .semaphore_type(vk::SemaphoreType::TIMELINE)
                .initial_value(0);
            let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
            let create_info = vk::SemaphoreCreateInfo::default()
                .push_next(&mut type_info)
                .push_next(&mut export_info);

            let timeline_semaphore = vk_device
                .create_semaphore(&create_info, None)
                .map_err(|err| {
                    InteropError::Vulkan(format!("create_semaphore(timeline): {err}"))
                })?;

            let external_semaphore_fd =
                ash::khr::external_semaphore_fd::Device::new(&vk_instance, &vk_device);

            (vk_device, external_semaphore_fd, timeline_semaphore)
        };

        Ok(Self {
            vk_device,
            timeline_semaphore,
            external_semaphore_fd,
            next_value: AtomicU64::new(0),
        })
    }
```

- [ ] **Step 2: Verify compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 3: The BackendMismatch test still passes (no new test for the success path — that requires a real Vulkan device, exercised in the smoke).**

Run: `cargo test -p servo-paint --lib interop::vulkan_timeline -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/interop/vulkan_timeline.rs
git commit -m "$(cat <<'EOF'
paint: VulkanTimelineSemaphoreSynchronizer::new — real Vulkan path

Pulls ash::Device + Instance from wgpu-hal's Vulkan accessors,
creates a timeline VkSemaphore with the export-fd hint baked in at
creation (spec requires it for vkGetSemaphoreFdKHR), and stashes the
external_semaphore_fd extension loader. Same wgpu-hal extraction
pattern graft uses in sync_vulkan::new.

GPU-touching path is exercised through the Phase 8 smoke runner;
isolated unit testing of vkCreateSemaphore would require a fixture
wgpu Vulkan device the workspace doesn't currently provision in unit
tests.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.3: Implement accessors + monotonic counter test

**Files:**
- Modify: `components/paint/interop/vulkan_timeline.rs`

- [ ] **Step 1: Add inherent-method accessors after `new`.**

```rust
    /// The Vulkan semaphore handle. Producers wire this into their own
    /// `VkSubmitInfo.pSignalSemaphores`; consumers into `pWaitSemaphores`.
    pub fn semaphore(&self) -> vk::Semaphore {
        self.timeline_semaphore
    }

    /// The wgpu Vulkan device the semaphore lives on. Callers issuing
    /// their own `vkQueueSubmit` need this to validate device match.
    pub fn device(&self) -> &ash::Device {
        &self.vk_device
    }

    /// Reserve the next value the producer should signal at. Monotonic
    /// across threads. Pure bookkeeping — does not change the GPU-side
    /// semaphore value. The producer integrating this into its own
    /// `pSignalSemaphoreValues` is what moves the GPU view.
    pub fn next_value(&self) -> u64 {
        self.next_value.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Highest value reserved by [`next_value`]. Snapshot.
    pub fn reserved_value(&self) -> u64 {
        self.next_value.load(Ordering::SeqCst)
    }
```

- [ ] **Step 2: Add the monotonic test.**

In `mod tests`:

```rust
    /// Multi-threaded monotonic-reserve smoke. Verifies `next_value`
    /// hands out disjoint, monotonically-increasing values under
    /// contention.
    #[test]
    fn next_value_monotonic_across_threads() {
        // We can't construct the synchronizer without a Vulkan device,
        // so test the atomic surface directly via an `AtomicU64` that
        // mirrors the wrapper's increment shape. (When the smoke runs
        // and constructs the real sync, this same pattern applies; the
        // unit test guards against a regression in the atomic-counter
        // discipline.)
        use std::sync::Arc;
        use std::thread;

        let counter = Arc::new(AtomicU64::new(0));
        let mut handles = vec![];
        for _ in 0..8 {
            let c = counter.clone();
            handles.push(thread::spawn(move || {
                let mut local = vec![];
                for _ in 0..1000 {
                    local.push(c.fetch_add(1, Ordering::SeqCst) + 1);
                }
                local
            }));
        }

        let mut all_values = vec![];
        for h in handles {
            all_values.extend(h.join().expect("thread joined"));
        }
        all_values.sort();
        assert_eq!(all_values.len(), 8 * 1000);
        // Disjoint: every value 1..=8000 appears exactly once.
        for (i, v) in all_values.iter().enumerate() {
            assert_eq!(*v, (i + 1) as u64);
        }
    }
```

- [ ] **Step 3: Run tests.**

Run: `cargo test -p servo-paint --lib interop::vulkan_timeline -- --nocapture`
Expected: PASS (both tests).

- [ ] **Step 4: Commit.**

```bash
git add components/paint/interop/vulkan_timeline.rs
git commit -m "$(cat <<'EOF'
paint: VulkanTimelineSemaphoreSynchronizer — accessors + monotonic test

Adds semaphore() / device() / next_value() / reserved_value()
inherent-method accessors. next_value is the producer's value-reserve
hook; the wrapper does not signal — producers integrate the handle
into their own VkSubmitInfo.

Multi-threaded monotonic test guards the atomic-counter discipline;
no GPU path required.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.4: Implement signaled_value / wait_host / export_fd + Drop

**Files:**
- Modify: `components/paint/interop/vulkan_timeline.rs`

- [ ] **Step 1: Add the GPU-touching accessors.**

After the existing accessors:

```rust
    /// Live host-readable semaphore value (`vkGetSemaphoreCounterValue`).
    /// This is the GPU's view — distinct from [`reserved_value`], which
    /// is the client-side bookkeeping atomic.
    pub fn signaled_value(&self) -> Result<u64, InteropError> {
        unsafe {
            self.vk_device
                .get_semaphore_counter_value(self.timeline_semaphore)
                .map_err(|err| InteropError::Vulkan(format!("get_semaphore_counter_value: {err}")))
        }
    }

    /// Block the calling thread until the timeline reaches `value`.
    /// `timeout_ns` is the per-Vulkan-spec nanosecond timeout
    /// (`u64::MAX` = wait forever).
    pub fn wait_host(&self, value: u64, timeout_ns: u64) -> Result<(), InteropError> {
        let semaphores = [self.timeline_semaphore];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .flags(vk::SemaphoreWaitFlags::empty())
            .semaphores(&semaphores)
            .values(&values);
        unsafe {
            self.vk_device
                .wait_semaphores(&info, timeout_ns)
                .map_err(|err| InteropError::Vulkan(format!("wait_semaphores({value}): {err}")))?;
        }
        Ok(())
    }

    /// Export the semaphore as an OPAQUE_FD for cross-process /
    /// external-driver consumers. Each call duplicates the fd; the
    /// caller owns the returned [`OwnedFd`].
    pub fn export_fd(&self) -> Result<OwnedFd, InteropError> {
        let info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(self.timeline_semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);
        let raw_fd = unsafe {
            self.external_semaphore_fd
                .get_semaphore_fd(&info)
                .map_err(|err| InteropError::Vulkan(format!("get_semaphore_fd: {err}")))?
        };
        // SAFETY: ash returned a fresh fd we own; wrap into OwnedFd so
        // the caller gets RAII close.
        Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
    }
```

- [ ] **Step 2: Add the Drop impl.**

```rust
impl Drop for VulkanTimelineSemaphoreSynchronizer {
    fn drop(&mut self) {
        unsafe { self.vk_device.destroy_semaphore(self.timeline_semaphore, None) };
    }
}
```

- [ ] **Step 3: Verify compile + tests.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo test -p servo-paint --lib interop::vulkan_timeline -- --nocapture`
Expected: PASS (both tests still green).

Run: `cargo clippy -p servo-paint -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/interop/vulkan_timeline.rs
git commit -m "$(cat <<'EOF'
paint: VulkanTimelineSemaphoreSynchronizer — host wait, export, Drop

Adds signaled_value() (vkGetSemaphoreCounterValue — GPU's view of the
timeline), wait_host(value, timeout_ns) (vkWaitSemaphores — native
host-side wait), and export_fd() (vkGetSemaphoreFdKHR with OPAQUE_FD
handle type, wrapped in OwnedFd for RAII).

Drop destroys the semaphore. The export_fd-returned OwnedFd is
independent of the wrapper's lifetime — the spec says each export is
a duplicate.

Fills the lineage-doc Linux synchronizer slot. Dormant on the smoke
path (same-queue FIFO); ready for cross-queue / cross-process consumers.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Compositor_wayland module structure

Promote the single-file skeleton to a module dir so subsequent commits land into focused submodules. Pure restructuring; no logic change yet. Spec §5.1.

### Task 3.1: Convert `compositor_wayland.rs` to `compositor_wayland/` module dir

**Files:**
- Delete: `components/paint/compositor_wayland.rs`
- Create: `components/paint/compositor_wayland/mod.rs`
- Create: `components/paint/compositor_wayland/errors.rs`

- [ ] **Step 1: Create `errors.rs` with the existing `BackendError` enum.**

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Errors raised by `WaylandSubsurfaceBackend`.

use crate::interop::InteropBackend;

/// Errors raised by [`crate::compositor_wayland::WaylandSubsurfaceBackend`]
/// construction or per-frame operations.
#[derive(Debug)]
pub enum BackendError {
    /// The supplied host wgpu context is not running on Vulkan.
    WrongBackend(InteropBackend),
    /// The provided wl_display pointer was null.
    NullDisplay,
    /// The provided wl_surface pointer was null.
    NullSurface,
    /// A Wayland registry global the backend requires was not advertised
    /// by the compositor.
    MissingGlobal(&'static str),
    /// No `(DRM format, modifier)` pair is supported by both Vulkan
    /// (RADV) and the Wayland compositor.
    NoCompatibleFormat,
    /// A Vulkan call failed during dmabuf import setup.
    Dmabuf(String),
    /// A Wayland protocol call failed.
    Wayland(String),
    /// The interop synchronizer could not be constructed.
    SyncInit(String),
    /// A path that hasn't been wired yet — see the named area.
    Unwired(&'static str),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongBackend(b) => {
                write!(f, "WaylandSubsurfaceBackend requires Vulkan, found {b:?}")
            },
            Self::NullDisplay => f.write_str("WaylandSubsurfaceBackend: null wl_display"),
            Self::NullSurface => f.write_str("WaylandSubsurfaceBackend: null wl_surface"),
            Self::MissingGlobal(g) => {
                write!(f, "WaylandSubsurfaceBackend: missing Wayland global: {g}")
            },
            Self::NoCompatibleFormat => f.write_str(
                "WaylandSubsurfaceBackend: no (DRM format, modifier) pair supported by both \
                 the Vulkan device and the Wayland compositor",
            ),
            Self::Dmabuf(m) => write!(f, "WaylandSubsurfaceBackend: dmabuf setup failed: {m}"),
            Self::Wayland(m) => write!(f, "WaylandSubsurfaceBackend: wayland call failed: {m}"),
            Self::SyncInit(m) => write!(f, "WaylandSubsurfaceBackend: sync init failed: {m}"),
            Self::Unwired(area) => {
                write!(f, "WaylandSubsurfaceBackend: not yet wired: {area}")
            },
        }
    }
}

impl std::error::Error for BackendError {}
```

- [ ] **Step 2: Move the existing single-file backend into `mod.rs`.**

Read the existing `components/paint/compositor_wayland.rs` and move its body verbatim into `components/paint/compositor_wayland/mod.rs`, with these tweaks:

1. Remove the inline `BackendError` enum + impls (now in `errors.rs`).
2. Add `mod errors;` near the top.
3. Add `pub use errors::BackendError;`.
4. Leave everything else (the skeleton `WaylandSubsurfaceBackend` struct, `present_master` returning `Unwired`, the `OsCompositorBackend` impl) untouched — Phase 6 rewrites it.

The mod.rs head looks like:

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Linux Wayland subsurface `OsCompositorBackend` impl.
//!
//! [... existing module-level docs from the skeleton ...]

#![allow(unsafe_code)]
#![allow(dead_code)]

mod errors;

pub use errors::BackendError;

use rustc_hash::FxHashMap;
use wgpu::Texture;

use crate::compositor::OsCompositorBackend;
use crate::interop::{HostWgpuContext, InteropBackend, SyncMechanism};
use netrender_device::compositor::SurfaceKey;

// [... existing struct + impls verbatim, minus the BackendError block ...]
```

- [ ] **Step 3: Delete the old single-file `compositor_wayland.rs`.**

```bash
git rm components/paint/compositor_wayland.rs
```

- [ ] **Step 4: Verify the rename is a no-op semantically.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo clippy -p servo-paint -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit.**

```bash
git add components/paint/compositor_wayland/ components/paint/compositor_wayland.rs
git commit -m "$(cat <<'EOF'
paint: compositor_wayland — promote to module dir

Pure restructure: single-file skeleton becomes compositor_wayland/mod.rs;
BackendError moves to compositor_wayland/errors.rs (with expanded
variants — MissingGlobal, NoCompatibleFormat, Dmabuf, Wayland, SyncInit —
prepared for the substantive landings in Phase 4-6).

Mirrors the compositor_calayer/ module-dir refactor (10420ed6258).
No logic change; present_master still returns BackendError::Unwired.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Dmabuf module

Allocates dmabuf-exportable VkImages via ash, wraps as `wgpu::Texture`, constructs `wl_buffer`s, owns the per-surface buffer pool with release-event recycling. Spec §4.3 + §7.

### Task 4.1: `dmabuf.rs` scaffold — `PlaneLayout` + `ExportableImage` struct + stub `new`

**Files:**
- Create: `components/paint/compositor_wayland/dmabuf.rs`
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Create the scaffold.**

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Dmabuf-exportable VkImage allocation + wl_buffer pool.
//!
//! [`ExportableImage`] wraps a VkImage allocated with the
//! `VK_EXT_image_drm_format_modifier` + `VK_EXT_external_memory_dma_buf`
//! extensions chained at create-time, then handed back into wgpu via
//! `wgpu::hal::vulkan::Device::texture_from_raw` +
//! `Device::create_texture_from_hal::<Vulkan>`. The result is a
//! [`wgpu::Texture`] indistinguishable from a self-allocated one
//! whose underlying VkImage can be exported as a dmabuf fd via
//! `vkGetMemoryFdKHR`.
//!
//! [`SurfaceBufferPool`] holds N=2 `wl_buffer`s constructed from
//! `ExportableImage`s, recycled via `wl_buffer.release` events.

#![allow(unsafe_op_in_unsafe_fn)]

use std::os::fd::OwnedFd;

use ash::vk;
use smallvec::SmallVec;

use crate::interop::HostWgpuContext;
use crate::compositor_wayland::errors::BackendError;

/// Single-plane layout from `vkGetImageSubresourceLayout`. For
/// `DRM_FORMAT_MOD_LINEAR` and most common modifiers, plane count is 1.
#[derive(Clone, Copy, Debug)]
pub struct PlaneLayout {
    pub offset: u64,
    pub pitch: u64,
}

/// DRM fourcc for `ABGR8888` (Vulkan `R8G8B8A8_UNORM` little-endian).
pub const DRM_FORMAT_ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
/// `DRM_FORMAT_MOD_LINEAR`.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// Dmabuf-exportable VkImage + memory + plane layout + the wgpu wrapper.
pub struct ExportableImage {
    vk_device: ash::Device,
    vk_image: vk::Image,
    vk_memory: vk::DeviceMemory,
    pub dmabuf_fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    pub drm_format: u32,
    pub drm_modifier: u64,
    pub planes: SmallVec<[PlaneLayout; 1]>,
    pub wgpu_texture: wgpu::Texture,
}

impl ExportableImage {
    /// Allocate an `R8G8B8A8_UNORM` image of `width × height` with the
    /// given DRM modifier, export the dmabuf fd, and wrap the VkImage
    /// back into a `wgpu::Texture`.
    ///
    /// Real impl lands in 4.2.
    pub fn new(
        host: &HostWgpuContext,
        width: u32,
        height: u32,
        drm_modifier: u64,
    ) -> Result<Self, BackendError> {
        let _ = (host, width, height, drm_modifier);
        Err(BackendError::Unwired("ExportableImage::new"))
    }
}

impl Drop for ExportableImage {
    fn drop(&mut self) {
        // wgpu_texture drops first (the create_texture_from_hal
        // callback owns the VkImage cleanup); any residual vk_image /
        // vk_memory left behind by an early panic during new() are
        // cleaned here.
        unsafe {
            if self.vk_image != vk::Image::null() {
                self.vk_device.destroy_image(self.vk_image, None);
            }
            if self.vk_memory != vk::DeviceMemory::null() {
                self.vk_device.free_memory(self.vk_memory, None);
            }
        }
    }
}
```

- [ ] **Step 2: Wire `dmabuf` into `mod.rs`.**

In `components/paint/compositor_wayland/mod.rs`, near `mod errors;`:

```rust
mod errors;
mod dmabuf;

pub use errors::BackendError;
```

- [ ] **Step 3: Add `smallvec` to the Linux-target deps.**

In `components/paint/Cargo.toml`'s Linux block:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
ash = "0.38"
smallvec = { workspace = true }
wayland-client = "0.31"
wayland-protocols = { version = "0.32", features = ["client", "unstable", "staging"] }
```

(`smallvec` is already a workspace dep — confirm by grepping.)

Run: `grep -n '^smallvec' Cargo.toml`
Expected: a workspace entry; if absent, add `smallvec = "1"` to `[workspace.dependencies]`.

- [ ] **Step 4: Verify compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 5: Commit.**

```bash
git add components/paint/Cargo.toml components/paint/compositor_wayland/dmabuf.rs components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: compositor_wayland/dmabuf.rs — scaffold ExportableImage

Types-only scaffold for the dmabuf-exportable VkImage wrapper.
`new()` returns Unwired; real allocation path lands next. Drop
defensively cleans residual VkImage/VkMemory in case construction
fails partway.

Adds DRM_FORMAT_ABGR8888 + DRM_FORMAT_MOD_LINEAR constants; PlaneLayout
helper struct. smallvec added as a Linux-target dep (already in the
workspace).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.2: Implement `ExportableImage::new` — VkImage + memory + fd export + wgpu wrap

**Files:**
- Modify: `components/paint/compositor_wayland/dmabuf.rs`

- [ ] **Step 1: Implement the allocation path.**

Replace the stub `new` body:

```rust
    pub fn new(
        host: &HostWgpuContext,
        width: u32,
        height: u32,
        drm_modifier: u64,
    ) -> Result<Self, BackendError> {
        let (vk_device, vk_image, vk_memory, dmabuf_fd, planes) = unsafe {
            let hal_device = host
                .device
                .as_hal::<wgpu::wgc::api::Vulkan>()
                .ok_or_else(|| BackendError::Dmabuf("wgpu-hal Vulkan device unavailable".into()))?;
            let vk_device = hal_device.raw_device().clone();
            let vk_instance = hal_device.shared_instance().raw_instance().clone();
            let vk_phys = hal_device.raw_physical_device();
            drop(hal_device);

            let external_memory_fd =
                ash::khr::external_memory_fd::Device::new(&vk_instance, &vk_device);
            let image_drm_modifier =
                ash::ext::image_drm_format_modifier::Device::new(&vk_instance, &vk_device);

            // ---- VkImage with the dmabuf + modifier chain ----------
            let modifier_list = [drm_modifier];
            let mut modifier_info = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
                .drm_format_modifiers(&modifier_list);
            let mut external_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let image_create_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D { width, height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::COLOR_ATTACHMENT,
                )
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .push_next(&mut external_info)
                .push_next(&mut modifier_info);

            let vk_image = vk_device
                .create_image(&image_create_info, None)
                .map_err(|e| BackendError::Dmabuf(format!("create_image: {e}")))?;

            // ---- Memory allocation with export hint ----------------
            let mem_req = vk_device.get_image_memory_requirements(vk_image);
            let mem_props = vk_instance.get_physical_device_memory_properties(vk_phys);
            let mem_type_index = pick_memory_type(
                &mem_props,
                mem_req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| BackendError::Dmabuf("no compatible memory type".into()))?;

            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(vk_image);
            let mut export_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let alloc_info = vk::MemoryAllocateInfo::default()
                .allocation_size(mem_req.size)
                .memory_type_index(mem_type_index)
                .push_next(&mut dedicated)
                .push_next(&mut export_info);

            let vk_memory = vk_device
                .allocate_memory(&alloc_info, None)
                .map_err(|e| {
                    vk_device.destroy_image(vk_image, None);
                    BackendError::Dmabuf(format!("allocate_memory: {e}"))
                })?;

            vk_device
                .bind_image_memory(vk_image, vk_memory, 0)
                .map_err(|e| {
                    vk_device.free_memory(vk_memory, None);
                    vk_device.destroy_image(vk_image, None);
                    BackendError::Dmabuf(format!("bind_image_memory: {e}"))
                })?;

            // ---- Export the dmabuf fd ------------------------------
            let get_fd_info = vk::MemoryGetFdInfoKHR::default()
                .memory(vk_memory)
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let raw_fd = external_memory_fd
                .get_memory_fd(&get_fd_info)
                .map_err(|e| {
                    vk_device.free_memory(vk_memory, None);
                    vk_device.destroy_image(vk_image, None);
                    BackendError::Dmabuf(format!("get_memory_fd: {e}"))
                })?;
            use std::os::fd::FromRawFd;
            let dmabuf_fd = OwnedFd::from_raw_fd(raw_fd);

            // ---- Plane layout via the modifier-properties ext ------
            let mut mod_props = vk::ImageDrmFormatModifierPropertiesEXT::default();
            image_drm_modifier
                .get_image_drm_format_modifier_properties(vk_image, &mut mod_props)
                .map_err(|e| {
                    BackendError::Dmabuf(format!("get_image_drm_format_modifier_properties: {e}"))
                })?;

            // For LINEAR-only v1, plane count is 1. Multi-plane modifiers
            // (when the picker promotes to tile-preferred) will need a
            // plane_count query — left as a Phase-7 follow-up.
            let aspect = vk::ImageAspectFlags::MEMORY_PLANE_0_EXT;
            let subresource = vk::ImageSubresource::default()
                .aspect_mask(aspect)
                .mip_level(0)
                .array_layer(0);
            let layout = vk_device.get_image_subresource_layout(vk_image, subresource);
            let planes = SmallVec::from_slice(&[PlaneLayout {
                offset: layout.offset,
                pitch: layout.row_pitch,
            }]);

            (vk_device, vk_image, vk_memory, dmabuf_fd, planes)
        };

        // ---- Wrap as wgpu::Texture via wgpu-hal --------------------
        let wgpu_texture = wrap_vk_image_as_wgpu(
            host,
            &vk_device,
            vk_image,
            vk_memory,
            width,
            height,
        )?;

        Ok(Self {
            vk_device,
            vk_image: vk::Image::null(), // ownership moved to wgpu wrapper's drop callback
            vk_memory: vk::DeviceMemory::null(),
            dmabuf_fd,
            width,
            height,
            drm_format: DRM_FORMAT_ABGR8888,
            drm_modifier,
            planes,
            wgpu_texture,
        })
    }
```

- [ ] **Step 2: Add the helpers below `impl Drop`.**

```rust
fn pick_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Option<u32> {
    for i in 0..props.memory_type_count {
        let suitable = type_bits & (1 << i) != 0;
        let flags = props.memory_types[i as usize].property_flags;
        if suitable && flags.contains(required) {
            return Some(i);
        }
    }
    None
}

fn wrap_vk_image_as_wgpu(
    host: &HostWgpuContext,
    vk_device: &ash::Device,
    vk_image: vk::Image,
    vk_memory: vk::DeviceMemory,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, BackendError> {
    use std::sync::Arc;

    // Drop callback: wgpu invokes this when the wgpu::Texture's last
    // ref drops. Destroys the image, frees the memory.
    let device_for_drop = vk_device.clone();
    let drop_image = vk_image;
    let drop_memory = vk_memory;
    let drop_callback: Box<dyn FnOnce() + Send + Sync + 'static> = Box::new(move || {
        unsafe {
            device_for_drop.destroy_image(drop_image, None);
            device_for_drop.free_memory(drop_memory, None);
        }
    });

    let hal_descriptor = wgpu::hal::TextureDescriptor {
        label: Some("ExportableImage dmabuf"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUses::COPY_DST
            | wgpu::TextureUses::RESOURCE
            | wgpu::TextureUses::COLOR_TARGET,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: vec![],
    };

    let hal_texture = unsafe {
        <wgpu::hal::api::Vulkan as wgpu::hal::Api>::Device::texture_from_raw(
            vk_image,
            &hal_descriptor,
            Some(drop_callback),
        )
    };

    let wgpu_texture = unsafe {
        host.device.create_texture_from_hal::<wgpu::wgc::api::Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("ExportableImage dmabuf"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        )
    };

    Ok(wgpu_texture)
}
```

- [ ] **Step 3: Verify compile.**

Run: `cargo check -p servo-paint`
Expected: success. If `wgpu::hal::vulkan::Device::texture_from_raw`'s exact signature differs in wgpu 29 (the drop-callback flavor changed across versions), adjust to match wgpu 29's actual API — the wgpu-hal docs at `target/doc/wgpu_hal/vulkan/struct.Device.html#method.texture_from_raw` after a build are the authoritative reference. The shape above is the wgpu 28→29 shape; if the descriptor field names drift, the error will say which.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/dmabuf.rs
git commit -m "$(cat <<'EOF'
paint: ExportableImage::new — VkImage alloc, fd export, wgpu wrap

Allocates an R8G8B8A8_UNORM VkImage with the
VK_EXT_image_drm_format_modifier + DMA_BUF_EXT external-memory
chain at create-time, allocates dedicated DEVICE_LOCAL memory with
the export hint, exports the dmabuf fd via vkGetMemoryFdKHR, queries
the plane layout via vkGetImageDrmFormatModifierPropertiesEXT +
vkGetImageSubresourceLayout (single-plane for LINEAR), and wraps the
VkImage back into a wgpu::Texture via wgpu-hal's texture_from_raw
with a drop-callback that owns the VkImage/VkMemory cleanup.

The wgpu::Texture is indistinguishable from a self-allocated one for
copy / render-pass use; its underlying VkImage can be dmabuf-imported
by Wayland via the exported fd.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.3: `ModifierTable` — query Vulkan importable set + intersect with Wayland advertisement

**Files:**
- Modify: `components/paint/compositor_wayland/dmabuf.rs`

- [ ] **Step 1: Add `ModifierTable`.**

Append:

```rust
/// `(format, modifier)` set the Wayland compositor advertised via
/// `zwp_linux_dmabuf_v1` events.
pub type WaylandAdvertised = Vec<(u32, u64)>;

/// Resolved choice picked by [`ModifierTable::choose`]. v1 always
/// chooses `(DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR)`; the
/// negotiation infrastructure stays in place so promoting to a
/// tile-preferred chooser later is a one-line change inside `choose`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChosenModifier {
    pub drm_format: u32,
    pub drm_modifier: u64,
}

pub struct ModifierTable {
    /// Compositor advertisement.
    advertised: WaylandAdvertised,
    /// Per-modifier Vulkan importability for ABGR8888.
    vulkan_importable: Vec<u64>,
}

impl ModifierTable {
    /// Query Vulkan's importable modifier set for `ABGR8888` and intersect
    /// with the compositor's advertised set. Stores both so the choice
    /// can be re-derived if the picker policy changes.
    pub fn new(
        host: &HostWgpuContext,
        advertised: WaylandAdvertised,
    ) -> Result<Self, BackendError> {
        let vulkan_importable = unsafe {
            let hal_device = host
                .device
                .as_hal::<wgpu::wgc::api::Vulkan>()
                .ok_or_else(|| BackendError::Dmabuf("wgpu-hal Vulkan device unavailable".into()))?;
            let vk_instance = hal_device.shared_instance().raw_instance().clone();
            let vk_phys = hal_device.raw_physical_device();
            drop(hal_device);

            query_importable_modifiers(&vk_instance, vk_phys)
        };

        Ok(Self {
            advertised,
            vulkan_importable,
        })
    }

    /// Pick the `(format, modifier)` to allocate against. v1: hard-codes
    /// LINEAR after verifying both Vulkan and the compositor agree on it.
    /// Errors with `NoCompatibleFormat` otherwise.
    pub fn choose(&self) -> Result<ChosenModifier, BackendError> {
        let advertised_linear = self
            .advertised
            .iter()
            .any(|(f, m)| *f == DRM_FORMAT_ABGR8888 && *m == DRM_FORMAT_MOD_LINEAR);
        let vk_linear = self.vulkan_importable.contains(&DRM_FORMAT_MOD_LINEAR);
        if !advertised_linear || !vk_linear {
            return Err(BackendError::NoCompatibleFormat);
        }
        Ok(ChosenModifier {
            drm_format: DRM_FORMAT_ABGR8888,
            drm_modifier: DRM_FORMAT_MOD_LINEAR,
        })
    }
}

unsafe fn query_importable_modifiers(
    instance: &ash::Instance,
    phys: vk::PhysicalDevice,
) -> Vec<u64> {
    // VkDrmFormatModifierPropertiesListEXT chained on
    // VkFormatProperties2 returns the device's known modifiers for
    // R8G8B8A8_UNORM. The two-call query (first count, then alloc + fill)
    // is the standard ash idiom.
    let mut count_props = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut fmt_props2 = vk::FormatProperties2::default().push_next(&mut count_props);
    instance.get_physical_device_format_properties2(phys, vk::Format::R8G8B8A8_UNORM, &mut fmt_props2);

    let n = count_props.drm_format_modifier_count as usize;
    if n == 0 {
        return Vec::new();
    }
    let mut buf: Vec<vk::DrmFormatModifierPropertiesEXT> =
        vec![vk::DrmFormatModifierPropertiesEXT::default(); n];
    let mut filled_props = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut buf);
    let mut fmt_props2 = vk::FormatProperties2::default().push_next(&mut filled_props);
    instance.get_physical_device_format_properties2(phys, vk::Format::R8G8B8A8_UNORM, &mut fmt_props2);

    buf.into_iter().map(|p| p.drm_format_modifier).collect()
}
```

- [ ] **Step 2: Unit-test the chooser predicate.**

In a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_table(advertised: WaylandAdvertised, vulkan_importable: Vec<u64>) -> ModifierTable {
        ModifierTable { advertised, vulkan_importable }
    }

    #[test]
    fn choose_picks_linear_when_both_advertise_it() {
        let t = fake_table(
            vec![(DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR)],
            vec![DRM_FORMAT_MOD_LINEAR],
        );
        assert_eq!(
            t.choose().unwrap(),
            ChosenModifier {
                drm_format: DRM_FORMAT_ABGR8888,
                drm_modifier: DRM_FORMAT_MOD_LINEAR,
            }
        );
    }

    #[test]
    fn choose_errors_when_vulkan_lacks_linear() {
        let t = fake_table(
            vec![(DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR)],
            vec![0xFFFF_FFFF_FFFF_0001], // some tile modifier, no LINEAR
        );
        assert!(matches!(t.choose(), Err(BackendError::NoCompatibleFormat)));
    }

    #[test]
    fn choose_errors_when_wayland_lacks_abgr_linear() {
        let t = fake_table(
            vec![(DRM_FORMAT_ABGR8888, 0xFFFF_FFFF_FFFF_0001)], // only tile, no LINEAR
            vec![DRM_FORMAT_MOD_LINEAR],
        );
        assert!(matches!(t.choose(), Err(BackendError::NoCompatibleFormat)));
    }
}
```

- [ ] **Step 3: Run tests + compile.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo test -p servo-paint --lib compositor_wayland::dmabuf::tests -- --nocapture`
Expected: 3/3 PASS.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/dmabuf.rs
git commit -m "$(cat <<'EOF'
paint: dmabuf ModifierTable — Vulkan + Wayland intersection

Queries the Vulkan importable modifier set for ABGR8888 via
VkDrmFormatModifierPropertiesListEXT chained on VkFormatProperties2.
Stores the compositor's advertised (format, modifier) pairs alongside.

`choose` is LINEAR-only on v1: verifies both Vulkan and the compositor
agree on LINEAR, errors with NoCompatibleFormat otherwise. The
infrastructure for tile-preferred picking is in place; promoting later
is a one-line change inside `choose`.

Three predicate tests cover the picker decision matrix.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.4: `SurfaceBufferPool` — N=2 wl_buffer pool + release-event recycling

**Files:**
- Modify: `components/paint/compositor_wayland/dmabuf.rs`

- [ ] **Step 1: Define the pool types.**

Append:

```rust
use std::sync::{Arc, Mutex};

use wayland_client::protocol::wl_buffer::WlBuffer;

/// Per-slot user data attached to each `wl_buffer`. The wayland-client
/// dispatcher uses this to find the matching slot on a release event.
#[derive(Clone, Debug)]
pub struct BufferSlotUserData {
    pub surface_id: u64,
    pub slot_index: u8,
    pub in_flight: Arc<Mutex<bool>>,
}

/// Per-surface `wl_buffer` pool. N=2 (mailbox) — what mainstream
/// Wayland clients use.
pub struct SurfaceBufferPool {
    pub width: u32,
    pub height: u32,
    pub chosen: ChosenModifier,
    pub slots: [BufferSlot; 2],
}

pub struct BufferSlot {
    pub image: ExportableImage,
    pub wl_buffer: WlBuffer,
    pub in_flight: Arc<Mutex<bool>>,
}

impl SurfaceBufferPool {
    /// Take the first `!in_flight` slot. Marks it `in_flight = true`.
    /// Returns the slot index + a reference to the wl_buffer + the
    /// wgpu::Texture for the encoder.
    pub fn acquire(&mut self) -> Option<usize> {
        for (i, slot) in self.slots.iter().enumerate() {
            let mut g = slot.in_flight.lock().expect("in_flight mutex");
            if !*g {
                *g = true;
                return Some(i);
            }
        }
        None
    }

    /// Whether at least one slot is available without an event roundtrip.
    pub fn has_available(&self) -> bool {
        self.slots
            .iter()
            .any(|s| !*s.in_flight.lock().expect("in_flight mutex"))
    }
}

impl Drop for SurfaceBufferPool {
    fn drop(&mut self) {
        for slot in &self.slots {
            slot.wl_buffer.destroy();
        }
    }
}
```

- [ ] **Step 2: Add a unit test for the predicate.**

```rust
    #[test]
    fn acquire_picks_first_available_then_blocks() {
        let in_flight_a = Arc::new(Mutex::new(false));
        let in_flight_b = Arc::new(Mutex::new(false));
        // Constructing a real BufferSlot requires a wl_buffer; the
        // acquire predicate operates on the in_flight Mutex slice alone.
        // Test the predicate directly.
        let bools = [in_flight_a.clone(), in_flight_b.clone()];
        fn first_available(bools: &[Arc<Mutex<bool>>; 2]) -> Option<usize> {
            for (i, b) in bools.iter().enumerate() {
                let mut g = b.lock().unwrap();
                if !*g {
                    *g = true;
                    return Some(i);
                }
            }
            None
        }
        assert_eq!(first_available(&bools), Some(0));
        assert_eq!(first_available(&bools), Some(1));
        assert_eq!(first_available(&bools), None);
        *in_flight_a.lock().unwrap() = false;
        assert_eq!(first_available(&bools), Some(0));
    }
```

- [ ] **Step 3: Verify compile + tests.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo test -p servo-paint --lib compositor_wayland::dmabuf::tests -- --nocapture`
Expected: 4/4 PASS.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/dmabuf.rs
git commit -m "$(cat <<'EOF'
paint: dmabuf SurfaceBufferPool — N=2 mailbox + release-event hook

Per-surface wl_buffer pool with two slots, each holding an
ExportableImage + WlBuffer + an Arc<Mutex<bool>> in_flight marker.

The in_flight Arc is also carried in BufferSlotUserData so the
wayland-client Dispatch<WlBuffer, BufferSlotUserData> impl can flip
it back to false when a release event arrives — wiring lives in
wayland.rs.

Acquire picks the first available slot; falls back to roundtrip when
both are in flight (caller's concern; this struct just reports
has_available).

Drop destroys both wl_buffers.

Predicate test covers the picker decision; the full pool's wayland
interaction is exercised through the smoke.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Wayland module: connection + globals binding

Owns the wayland-client connection, registry, and per-frame event dispatch. Spec §4.1 + §6.1.

### Task 5.1: `wayland.rs` — Connection, registry, globals struct

**Files:**
- Create: `components/paint/compositor_wayland/wayland.rs`
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Create the wayland module.**

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Wayland connection + globals binding for the C4 backend.
//!
//! Wraps `wayland_client::Connection::from_ptr` over the embedder's
//! `wl_display`, runs `registry_queue_init` to bind the required globals
//! (`wl_compositor`, `wl_subcompositor`, `zwp_linux_dmabuf_v1`,
//! `wp_viewporter`) and the optional `wp_alpha_modifier_v1`, drains
//! the dmabuf format/modifier advertisements, and dispatches per-frame
//! events (notably `wl_buffer.release`).

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
// wp_alpha_modifier_v1 lives under wayland-protocols' staging tree.
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1;

use crate::compositor_wayland::dmabuf::{
    BufferSlotUserData, ChosenModifier, WaylandAdvertised, DRM_FORMAT_ABGR8888,
    DRM_FORMAT_MOD_LINEAR,
};
use crate::compositor_wayland::errors::BackendError;

/// Bound Wayland globals required by the backend.
pub struct WaylandGlobals {
    pub compositor: WlCompositor,
    pub subcompositor: WlSubcompositor,
    pub dmabuf: ZwpLinuxDmabufV1,
    pub viewporter: WpViewporter,
    pub alpha_modifier: Option<WpAlphaModifierV1>,
}

/// Lives in `WaylandSubsurfaceBackend`. Holds the connection + event
/// queue + bound globals + the modifier negotiation result.
pub struct WaylandState {
    pub connection: Connection,
    pub event_queue: EventQueue<DispatchState>,
    pub queue_handle: QueueHandle<DispatchState>,
    pub globals: WaylandGlobals,
    pub parent_surface: WlSurface,
    pub advertised: WaylandAdvertised,
    pub dispatch_state: DispatchState,
}

/// Dispatch user-data state held by the event queue. Keeps the
/// list of advertised `(format, modifier)` pairs current and provides
/// the buffer-release routing.
pub struct DispatchState {
    pub advertised: Arc<Mutex<WaylandAdvertised>>,
}

impl WaylandState {
    /// Build a state from raw `wl_display` + `wl_surface` pointers
    /// owned by the embedder. The connection borrows the display; the
    /// caller retains lifetime responsibility.
    ///
    /// # Safety
    ///
    /// `display` and `parent_surface` must point to live Wayland
    /// objects whose lifetime exceeds the returned state.
    pub unsafe fn new(
        display: *mut c_void,
        parent_surface_ptr: *mut c_void,
    ) -> Result<Self, BackendError> {
        if display.is_null() {
            return Err(BackendError::NullDisplay);
        }
        if parent_surface_ptr.is_null() {
            return Err(BackendError::NullSurface);
        }

        let connection = Connection::from_ptr(display.cast())
            .map_err(|e| BackendError::Wayland(format!("Connection::from_ptr: {e}")))?;

        let advertised = Arc::new(Mutex::new(WaylandAdvertised::new()));
        let dispatch_state = DispatchState {
            advertised: advertised.clone(),
        };

        let (globals, event_queue) = registry_queue_init::<DispatchState>(&connection)
            .map_err(|e| BackendError::Wayland(format!("registry_queue_init: {e}")))?;
        let queue_handle = event_queue.handle();

        let compositor: WlCompositor = globals
            .bind(&queue_handle, 4..=6, ())
            .map_err(|e| BackendError::Wayland(format!("bind wl_compositor: {e}")))?;
        let subcompositor: WlSubcompositor = globals
            .bind(&queue_handle, 1..=1, ())
            .map_err(|e| BackendError::Wayland(format!("bind wl_subcompositor: {e}")))?;
        let dmabuf: ZwpLinuxDmabufV1 = globals
            .bind(&queue_handle, 3..=4, ())
            .map_err(|_| BackendError::MissingGlobal("zwp_linux_dmabuf_v1"))?;
        let viewporter: WpViewporter = globals
            .bind(&queue_handle, 1..=1, ())
            .map_err(|_| BackendError::MissingGlobal("wp_viewporter"))?;
        let alpha_modifier: Option<WpAlphaModifierV1> =
            globals.bind(&queue_handle, 1..=1, ()).ok();

        let bound_globals = WaylandGlobals {
            compositor,
            subcompositor,
            dmabuf: dmabuf.clone(),
            viewporter,
            alpha_modifier,
        };

        // Adopt the embedder's parent wl_surface via from_external_id.
        // wayland-client doesn't have a public from_raw API for protocol
        // objects; the canonical path is `Proxy::from_id` against the
        // ObjectId obtained from a winit-provided pointer. raw-window-
        // handle's WaylandWindowHandle hands us the raw c-side wl_surface.
        let parent_surface = unsafe {
            wayland_client_adopt_surface(&connection, parent_surface_ptr)
                .map_err(|e| BackendError::Wayland(format!("adopt parent wl_surface: {e}")))?
        };

        let advertised_snapshot = advertised.lock().unwrap().clone();

        let mut state = Self {
            connection,
            event_queue,
            queue_handle,
            globals: bound_globals,
            parent_surface,
            advertised: advertised_snapshot,
            dispatch_state,
        };

        // Drive a roundtrip so the dmabuf format / modifier events arrive
        // before any caller asks for a chosen modifier.
        state
            .event_queue
            .roundtrip(&mut state.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("roundtrip(initial): {e}")))?;
        state.advertised = state.dispatch_state.advertised.lock().unwrap().clone();

        Ok(state)
    }

    /// Drain any pending events (notably `wl_buffer.release`) without
    /// blocking. Called at the top of `present_master` / `present`.
    pub fn dispatch_pending(&mut self) -> Result<(), BackendError> {
        self.event_queue
            .dispatch_pending(&mut self.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("dispatch_pending: {e}")))?;
        Ok(())
    }

    /// Block until at least one event is dispatched. Called when the
    /// buffer pool is starved.
    pub fn roundtrip(&mut self) -> Result<(), BackendError> {
        self.event_queue
            .roundtrip(&mut self.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("roundtrip: {e}")))?;
        Ok(())
    }

    /// Flush queued protocol messages to the compositor.
    pub fn flush(&mut self) -> Result<(), BackendError> {
        self.connection
            .flush()
            .map_err(|e| BackendError::Wayland(format!("flush: {e}")))?;
        Ok(())
    }
}

/// Helper that converts a raw `*mut wl_surface` (from raw-window-handle)
/// into a wayland-client `WlSurface` proxy. wayland-client 0.31 supports
/// this via `Proxy::from_id` with an `ObjectId::from_ptr`.
unsafe fn wayland_client_adopt_surface(
    connection: &Connection,
    raw: *mut c_void,
) -> Result<WlSurface, String> {
    use wayland_backend::client::ObjectId;
    use wayland_backend::sys::client::Backend;

    let backend = Backend::from_foreign_display(raw.cast());
    // Actually: ObjectId::from_ptr requires the backend's interface table
    // and the C pointer. wayland-client 0.31 exposes:
    //   ObjectId::from_ptr(interface, ptr) -> Result<ObjectId, InvalidId>
    // Then WlSurface::from_id(connection, object_id).
    let id = ObjectId::from_ptr(WlSurface::interface(), raw.cast())
        .map_err(|e| format!("ObjectId::from_ptr: {e:?}"))?;
    let surface = WlSurface::from_id(connection, id)
        .map_err(|e| format!("WlSurface::from_id: {e:?}"))?;
    let _ = backend;
    Ok(surface)
}

// ---- Dispatch impls --------------------------------------------------
// wayland-client requires Dispatch impls for every proxy whose events
// we want to handle. The default-no-op impls cover the globals we just
// bind-and-use; the meaningful one is WlBuffer for release-event routing
// and ZwpLinuxDmabufV1 for format/modifier advertisement.

macro_rules! noop_dispatch {
    ($proxy:ty) => {
        impl Dispatch<$proxy, ()> for DispatchState {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

noop_dispatch!(WlCompositor);
noop_dispatch!(WlSubcompositor);
noop_dispatch!(WpViewporter);
noop_dispatch!(WpAlphaModifierV1);
noop_dispatch!(WlSurface);

impl Dispatch<WlRegistry, GlobalListContents> for DispatchState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for DispatchState {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: <ZwpLinuxDmabufV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::Event;
        match event {
            Event::Format { format: _ } => {
                // v1 Format event is per-spec deprecated for modifier-
                // capable compositors; ignore and rely on Modifier.
            },
            Event::Modifier {
                format,
                modifier_hi,
                modifier_lo,
            } => {
                let modifier = ((modifier_hi as u64) << 32) | (modifier_lo as u64);
                state
                    .advertised
                    .lock()
                    .unwrap()
                    .push((format, modifier));
            },
            _ => {},
        }
    }
}

impl Dispatch<WlBuffer, BufferSlotUserData> for DispatchState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: <WlBuffer as Proxy>::Event,
        user_data: &BufferSlotUserData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_buffer::Event;
        if matches!(event, Event::Release) {
            let mut g = user_data.in_flight.lock().unwrap();
            *g = false;
        }
    }
}
```

- [ ] **Step 2: Wire `wayland.rs` into `mod.rs`.**

```rust
mod errors;
mod dmabuf;
mod wayland;

pub use errors::BackendError;
```

- [ ] **Step 3: Compile.**

Run: `cargo check -p servo-paint`
Expected: success. If wayland-client 0.31's `globals::registry_queue_init` signature or `Connection::from_ptr` semantics differ minorly, fix per the compiler's diagnostics — the wayland-client docs build with `cargo doc -p wayland-client` is the authoritative reference.

The `wayland_client_adopt_surface` helper is the most likely API-drift point; if `Backend::from_foreign_display` isn't the exact API, the alternative is `Connection::from_socket(raw_fd)` or, simpler, having winit's `RawWindowHandle::Wayland` give us a wayland-backend `WaylandHandle` we feed through `wayland_client::backend::ObjectId::from_ptr(WlSurface::interface(), surface_ptr)` directly without re-establishing a Backend.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/wayland.rs components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: compositor_wayland — wayland-client connection + globals binding

WaylandState wraps Connection::from_ptr over the embedder's wl_display,
runs registry_queue_init, binds required globals (wl_compositor v4-6,
wl_subcompositor v1, zwp_linux_dmabuf_v1 v3-4, wp_viewporter v1) and
the optional wp_alpha_modifier_v1, adopts the embedder's parent
wl_surface via Proxy::from_id, then roundtrips once so the dmabuf
format/modifier advertisements drain before the backend asks the
modifier table for a chosen pair.

Dispatch impls:
- No-op for the proxies whose events we don't care about (compositor,
  subcompositor, viewporter, alpha_modifier, surface, registry).
- ZwpLinuxDmabufV1: collects Modifier events into the advertised set.
- WlBuffer<BufferSlotUserData>: flips the slot's in_flight Arc<Mutex>
  back to false on Release.

dispatch_pending / roundtrip / flush exposed for per-frame use.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Backend per-frame body

Replaces the skeleton in `compositor_wayland/mod.rs` with the full implementation: connection wiring, master path, declare/present/destroy. Spec §6.

### Task 6.1: Rewrite `WaylandSubsurfaceBackend` struct + `new`

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Replace the skeleton struct + new().**

Replace the contents of `compositor_wayland/mod.rs` (preserving the module-level docs at the top) with the new layout. Here's the head and `new`:

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Linux Wayland subsurface `OsCompositorBackend` impl.
//!
//! [... module-level docs from the skeleton, updated to reflect the
//! landed state instead of "skeleton" ...]

#![allow(unsafe_code)]
#![allow(dead_code)]

mod bake;
mod dmabuf;
mod errors;
mod wayland;

pub use errors::BackendError;

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;
use wgpu::Texture;

use crate::compositor::OsCompositorBackend;
use crate::interop::{
    HostWgpuContext, InteropBackend, SyncMechanism, VulkanTimelineSemaphoreSynchronizer,
};
use netrender_device::compositor::SurfaceKey;

use bake::BakePipeline;
use dmabuf::{ChosenModifier, ExportableImage, ModifierTable, SurfaceBufferPool};
use wayland::WaylandState;

use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy;

use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

/// Linux Wayland subsurface compositor backend.
pub struct WaylandSubsurfaceBackend {
    host: HostWgpuContext,
    wayland: WaylandState,
    modifier_table: ModifierTable,
    chosen: ChosenModifier,
    bake_pipeline: BakePipeline,
    vk_timeline_sync: VulkanTimelineSemaphoreSynchronizer,

    surfaces: FxHashMap<SurfaceKey, WaylandSurface>,

    /// Side-buffer the master texture is blitted into per frame before
    /// dmabuf-attaching to the parent surface. Allocated lazily and
    /// reallocated on master-size change.
    master_side: Option<SurfaceBufferPool>,

    /// Monotonic generation for `BufferSlotUserData.surface_id`.
    /// The master uses id=0; per-`SurfaceKey` surfaces increment.
    next_surface_id: u64,
}

struct WaylandSurface {
    wl_surface: WlSurface,
    wl_subsurface: WlSubsurface,
    viewport: WpViewport,
    alpha_modifier: Option<WpAlphaModifierSurfaceV1>,
    surface_id: u64,
    source_dest: SurfaceBufferPool,
    bake: Option<SurfaceBufferPool>,
    size: (u32, u32),
}

unsafe impl Send for WaylandSubsurfaceBackend {}

impl WaylandSubsurfaceBackend {
    /// Construct the backend over the embedder's wayland display +
    /// surface. Both pointers must be non-null and outlive the backend.
    ///
    /// # Safety
    ///
    /// `display` must point to a valid `wl_display`; `parent_surface`
    /// to a valid `wl_surface`. Both ownerships stay with the caller;
    /// the backend only borrows.
    pub unsafe fn new(
        host: &HostWgpuContext,
        display: *mut c_void,
        parent_surface: *mut c_void,
    ) -> Result<Self, BackendError> {
        if host.backend != InteropBackend::Vulkan {
            return Err(BackendError::WrongBackend(host.backend));
        }

        let wayland = unsafe { WaylandState::new(display, parent_surface)? };
        let modifier_table =
            ModifierTable::new(host, wayland.advertised.clone())?;
        let chosen = modifier_table.choose()?;
        log::info!(
            "[WaylandSubsurfaceBackend] dmabuf modifier: format=0x{:08X} modifier=0x{:016X}",
            chosen.drm_format,
            chosen.drm_modifier,
        );

        let bake_pipeline = BakePipeline::new(&host.device);
        let vk_timeline_sync = VulkanTimelineSemaphoreSynchronizer::new(host)
            .map_err(|e| BackendError::SyncInit(format!("{e}")))?;

        Ok(Self {
            host: host.clone(),
            wayland,
            modifier_table,
            chosen,
            bake_pipeline,
            vk_timeline_sync,
            surfaces: FxHashMap::default(),
            master_side: None,
            next_surface_id: 0,
        })
    }
}
```

- [ ] **Step 2: Compile.**

Run: `cargo check -p servo-paint`
Expected: `BakePipeline::new` not found (defined in 7.1). Stub it for now: create `components/paint/compositor_wayland/bake.rs` with:

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Rotation + opacity bake pipeline. See `compositor_wayland/mod.rs`
//! for the gating predicate.

pub struct BakePipeline {
    // Real impl lands in Phase 7.
    _placeholder: (),
}

impl BakePipeline {
    pub fn new(_device: &wgpu::Device) -> Self {
        Self { _placeholder: () }
    }
}
```

Re-run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 3: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs components/paint/compositor_wayland/bake.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend — struct + new (wayland + modifier + sync)

Replaces the skeleton struct/new with the full construction: borrow
the embedder's wl_display/wl_surface via WaylandState::new, query the
modifier intersection via ModifierTable, log the chosen
(format, modifier), construct the dormant
VulkanTimelineSemaphoreSynchronizer, and prep an empty surfaces table.

BakePipeline lands as a stub here; real impl in Phase 7.

present_master / declare / present / destroy still return / no-op the
previous skeleton behavior — they're rewritten in 6.2-6.5.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 6.2: `present_master` — master → side buffer → parent surface

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Implement `present_master`.**

Add an inherent impl block:

```rust
impl WaylandSubsurfaceBackend {
    pub fn present_master(&mut self, master: &Texture) -> Result<(), BackendError> {
        self.wayland.dispatch_pending()?;

        let size = master.size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }

        // Ensure side-buffer pool sized to current master.
        let need_realloc = match &self.master_side {
            Some(p) => p.width != size.width || p.height != size.height,
            None => true,
        };
        if need_realloc {
            self.master_side = Some(self.allocate_pool(0, size.width, size.height)?);
        }
        let pool = self.master_side.as_mut().expect("just allocated");

        // Acquire a slot; if both in flight, roundtrip until one
        // releases.
        let slot_index = loop {
            if let Some(i) = pool.acquire() {
                break i;
            }
            self.wayland.roundtrip()?;
        };
        let slot = &pool.slots[slot_index];

        // Encode master -> side-buffer blit on wgpu's queue.
        let mut encoder =
            self.host
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("WaylandSubsurfaceBackend::present_master master→side"),
                });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: master,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &slot.image.wgpu_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        self.host.queue.submit([encoder.finish()]);

        // Attach to parent surface; damage; commit; flush.
        self.wayland
            .parent_surface
            .attach(Some(&slot.wl_buffer), 0, 0);
        self.wayland.parent_surface.damage_buffer(
            0,
            0,
            size.width as i32,
            size.height as i32,
        );
        self.wayland.parent_surface.commit();
        self.wayland.flush()?;

        Ok(())
    }
}
```

- [ ] **Step 2: Add the `allocate_pool` helper.**

In the same impl block, before `present_master`:

```rust
    fn allocate_pool(
        &mut self,
        surface_id: u64,
        width: u32,
        height: u32,
    ) -> Result<SurfaceBufferPool, BackendError> {
        let chosen = self.chosen;
        let slot0 = self.build_slot(surface_id, 0, width, height, chosen)?;
        let slot1 = self.build_slot(surface_id, 1, width, height, chosen)?;
        Ok(SurfaceBufferPool {
            width,
            height,
            chosen,
            slots: [slot0, slot1],
        })
    }

    fn build_slot(
        &self,
        surface_id: u64,
        slot_index: u8,
        width: u32,
        height: u32,
        chosen: ChosenModifier,
    ) -> Result<dmabuf::BufferSlot, BackendError> {
        let image = ExportableImage::new(&self.host, width, height, chosen.drm_modifier)?;
        let in_flight = Arc::new(Mutex::new(false));
        let user_data = dmabuf::BufferSlotUserData {
            surface_id,
            slot_index,
            in_flight: in_flight.clone(),
        };

        // Build wl_buffer via zwp_linux_dmabuf_v1.create_params() +
        // params.add() + params.create_immed().
        let params: ZwpLinuxBufferParamsV1 = self
            .wayland
            .globals
            .dmabuf
            .create_params(&self.wayland.queue_handle, ());
        let plane = image.planes[0];
        // dup the fd so the wayland-side close doesn't disturb the
        // Vulkan-side memory.
        let dup_fd = image
            .dmabuf_fd
            .try_clone()
            .map_err(|e| BackendError::Dmabuf(format!("dup fd: {e}")))?;
        params.add(
            dup_fd.into_raw_fd(),
            0,                            // plane_idx
            plane.offset as u32,
            plane.pitch as u32,
            (chosen.drm_modifier >> 32) as u32,
            chosen.drm_modifier as u32,
        );
        let wl_buffer = params.create_immed(
            width as i32,
            height as i32,
            chosen.drm_format,
            wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Flags::empty(),
            &self.wayland.queue_handle,
            user_data,
        );

        Ok(dmabuf::BufferSlot {
            image,
            wl_buffer,
            in_flight,
        })
    }
```

- [ ] **Step 3: Wire OsCompositorBackend::present_master.**

Find the `OsCompositorBackend` impl (still at the bottom of the file from Phase 3) and update its `present_master`:

```rust
    fn present_master(&mut self, master: &Texture) {
        if let Err(err) = WaylandSubsurfaceBackend::present_master(self, master) {
            log::warn!("[WaylandSubsurfaceBackend] present_master: {err}");
        }
    }
```

(Existing skeleton already had this shape; verify it's wired to the inherent impl, not still returning `Unwired`.)

- [ ] **Step 4: Add `IntoRawFd` import.**

At the top of `mod.rs`:

```rust
use std::os::fd::IntoRawFd;
```

- [ ] **Step 5: Compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 6: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend::present_master — master→side→parent

Per-frame body for the master path:
1. dispatch_pending to drain release events.
2. (Re)allocate master_side pool on size change (lazy / once).
3. acquire a slot; roundtrip if both in flight.
4. encode master→side-buffer copy_texture_to_texture; submit on wgpu's queue.
5. attach side wl_buffer to parent wl_surface; damage_buffer; commit; flush.

Helpers:
- allocate_pool / build_slot: per-(surface_id, dim) pool construction;
  creates the ExportableImage, builds the wl_buffer via
  zwp_linux_dmabuf_v1.create_params + params.add + create_immed,
  attaches BufferSlotUserData carrying the in_flight Arc<Mutex>.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 6.3: `declare` — per-key surface + subsurface + viewport + dest pool

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Implement `declare`.**

In the inherent impl:

```rust
    fn declare_inherent(
        &mut self,
        key: SurfaceKey,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture, BackendError> {
        if format != wgpu::TextureFormat::Rgba8Unorm {
            return Err(BackendError::Dmabuf(format!(
                "declare: unsupported format {format:?} (only Rgba8Unorm)"
            )));
        }

        self.next_surface_id += 1;
        let surface_id = self.next_surface_id;

        let source_dest = self.allocate_pool(surface_id, width, height)?;
        let dest_texture = source_dest.slots[0].image.wgpu_texture.clone();

        let wl_surface = self
            .wayland
            .globals
            .compositor
            .create_surface(&self.wayland.queue_handle, ());
        let wl_subsurface = self
            .wayland
            .globals
            .subcompositor
            .get_subsurface(
                &wl_surface,
                &self.wayland.parent_surface,
                &self.wayland.queue_handle,
                (),
            );
        wl_subsurface.set_desync();
        wl_subsurface.set_position(0, 0);

        let viewport = self
            .wayland
            .globals
            .viewporter
            .get_viewport(&wl_surface, &self.wayland.queue_handle, ());
        let alpha_modifier = self
            .wayland
            .globals
            .alpha_modifier
            .as_ref()
            .map(|am| am.get_surface(&wl_surface, &self.wayland.queue_handle, ()));

        self.surfaces.insert(
            key,
            WaylandSurface {
                wl_surface,
                wl_subsurface,
                viewport,
                alpha_modifier,
                surface_id,
                source_dest,
                bake: None,
                size: (width, height),
            },
        );

        // The ServoCompositor wrapper will allocate two slots' worth of
        // pool, but it only needs one wgpu::Texture handle to encode
        // master[rect]→dest into. Both slots reference the same
        // wgpu::Texture? No — the pool has two separate ExportableImages
        // each with its own wgpu::Texture, because the dmabuf wl_buffer
        // attach/release lifecycle is per-image. The compositor's blit
        // target rotates between them as acquire() picks slots in
        // present().
        //
        // We return slot 0's texture here so declare can be a one-shot
        // allocation, then present() figures out which slot is acquired
        // and ensures the blit target matches.
        //
        // Wait — that's structurally wrong. ServoCompositor expects ONE
        // destination texture per key (it allocates lazily in present_frame
        // and reuses it across frames). It blits master[rect] -> dest
        // every dirty frame, then calls backend.present(key, ...).
        //
        // Reconciling: declare allocates ONE ExportableImage as the dest
        // (returned), plus pool gets a SECOND ExportableImage that's used
        // as the swap-buffer. present() handles the swap by encoding a
        // copy from slot 0 → slot 1 when slot 0 is in flight. Tracked in
        // 6.4.

        Ok(dest_texture)
    }
```

Important: the comment block reflects a design subtlety surfaced during implementation. The clean resolution: **`ServoCompositor::present_frame` already blits `master[rect] → dest` per frame** (it does `copy_texture_to_texture` into the destination wgpu texture). The pool's two slots solve the *wl_buffer lifecycle* problem (one buffer attached, one being filled), not the wgpu texture problem.

The simplest correct shape: **the source_dest pool's `slots[i].image.wgpu_texture` is the same allocation**? No — each slot is a separate VkImage. So either:
- (a) Allocate ONE ExportableImage and reuse its wgpu_texture for both pool slots (only the wl_buffer wraps differ — but a wl_buffer is bound to a dmabuf fd, not to the image, so this would need separate dmabuf fds anyway, which means separate VkImages). Not viable.
- (b) ServoCompositor blits to slot N each frame; in `present()` we need to figure out which slot N is. This requires `declare` to return a wgpu::Texture that's actually a re-binding — not possible with wgpu's design.
- (c) Reduce to N=1 buffer (no pool): one VkImage, one wl_buffer, attach + commit synchronously per frame, accept the per-frame "wait for buffer.release" stall.
- (d) **Correct design**: `ServoCompositor` blits into the dest wgpu texture; then in `present()`, the backend acquires a pool slot and *copies dest → slot.image* before attaching. This is one extra blit per frame but cleanly decouples the wgpu-side dest texture (stable across frames) from the dmabuf-buffer lifecycle (two-slot mailbox).

Rewriting with (d):

- [ ] **Step 2: Replace `declare_inherent` with the (d) design — declare allocates one stable dest texture + a pool whose slots are swap targets.**

```rust
struct WaylandSurface {
    wl_surface: WlSurface,
    wl_subsurface: WlSubsurface,
    viewport: WpViewport,
    alpha_modifier: Option<WpAlphaModifierSurfaceV1>,
    surface_id: u64,
    /// Stable wgpu-side destination texture. ServoCompositor blits
    /// master[rect] → this every dirty frame.
    dest_texture: wgpu::Texture,
    /// Two-slot dmabuf pool. `present` copies dest_texture → acquired
    /// slot, then attaches the slot's wl_buffer.
    swap_pool: SurfaceBufferPool,
    /// Lazily allocated bake target (rotation / alpha-bake).
    bake: Option<SurfaceBufferPool>,
    size: (u32, u32),
}

impl WaylandSubsurfaceBackend {
    fn declare_inherent(
        &mut self,
        key: SurfaceKey,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture, BackendError> {
        if format != wgpu::TextureFormat::Rgba8Unorm {
            return Err(BackendError::Dmabuf(format!(
                "declare: unsupported format {format:?} (only Rgba8Unorm)"
            )));
        }

        self.next_surface_id += 1;
        let surface_id = self.next_surface_id;

        // Stable wgpu dest (not dmabuf-exportable — ServoCompositor's
        // blit target, copied into the swap_pool slots in present()).
        let dest_texture = self.host.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("WaylandSubsurfaceBackend dest"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let swap_pool = self.allocate_pool(surface_id, width, height)?;

        let wl_surface = self
            .wayland
            .globals
            .compositor
            .create_surface(&self.wayland.queue_handle, ());
        let wl_subsurface = self.wayland.globals.subcompositor.get_subsurface(
            &wl_surface,
            &self.wayland.parent_surface,
            &self.wayland.queue_handle,
            (),
        );
        wl_subsurface.set_desync();
        wl_subsurface.set_position(0, 0);

        let viewport = self
            .wayland
            .globals
            .viewporter
            .get_viewport(&wl_surface, &self.wayland.queue_handle, ());
        let alpha_modifier = self
            .wayland
            .globals
            .alpha_modifier
            .as_ref()
            .map(|am| am.get_surface(&wl_surface, &self.wayland.queue_handle, ()));

        self.surfaces.insert(
            key,
            WaylandSurface {
                wl_surface,
                wl_subsurface,
                viewport,
                alpha_modifier,
                surface_id,
                dest_texture: dest_texture.clone(),
                swap_pool,
                bake: None,
                size: (width, height),
            },
        );

        Ok(dest_texture)
    }
}
```

- [ ] **Step 3: Wire `OsCompositorBackend::declare`.**

In the trait impl:

```rust
    fn declare(
        &mut self,
        key: SurfaceKey,
        host: &HostWgpuContext,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture, crate::compositor::BoxedBackendError> {
        let _ = host; // declare uses self.host (set at construction)
        WaylandSubsurfaceBackend::declare_inherent(self, key, width, height, format)
            .map_err(|e| Box::new(e) as crate::compositor::BoxedBackendError)
    }
```

- [ ] **Step 4: Compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 5: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend::declare — per-key surface + dest + pool

Allocates a stable wgpu dest texture (ServoCompositor's blit target,
non-exportable, reused across frames), a two-slot dmabuf swap pool
(present() copies dest → acquired slot, then attaches), the per-key
wl_surface + wl_subsurface (parent = embedder parent_surface,
set_desync, position 0,0), the wp_viewport, and the optional
wp_alpha_modifier_surface_v1.

The dest_texture / swap_pool split keeps ServoCompositor's
copy_texture_to_texture API stable (one wgpu::Texture per key) while
the dmabuf wl_buffer lifecycle (acquire/release across two slots)
remains a backend-private concern.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 6.4: `present` fast path — viewport + alpha-modifier + dest→slot → attach

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Implement `present_fast_path`.**

```rust
impl WaylandSubsurfaceBackend {
    fn present_inherent(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) -> Result<(), BackendError> {
        self.wayland.dispatch_pending()?;

        let needs_rotation = transform[1].abs() > 1e-6 || transform[2].abs() > 1e-6;
        let needs_alpha_bake = !self.wayland.globals.alpha_modifier.is_some()
            && (opacity - 1.0).abs() > 1e-6;

        // Bake path lives in 6.5. Fast path:
        if needs_rotation || needs_alpha_bake {
            return self.present_baked_path(key, transform, clip, opacity);
        }

        let surface = self
            .surfaces
            .get_mut(&key)
            .ok_or_else(|| BackendError::Wayland(format!("present({key:?}): surface not declared")))?;

        // Acquire a swap slot; roundtrip if starved.
        let slot_index = loop {
            if let Some(i) = surface.swap_pool.acquire() {
                break i;
            }
            self.wayland.roundtrip()?;
        };
        let slot = &surface.swap_pool.slots[slot_index];

        // Copy dest → slot.image on wgpu's queue.
        let mut encoder =
            self.host
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("WaylandSubsurfaceBackend::present dest→slot"),
                });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surface.dest_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &slot.image.wgpu_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: surface.size.0,
                height: surface.size.1,
                depth_or_array_layers: 1,
            },
        );
        self.host.queue.submit([encoder.finish()]);

        // Viewport — source rect is full source-dest size; destination
        // size derived from transform scale.
        let m11 = transform[0];
        let m22 = transform[3];
        let dest_w = (surface.size.0 as f32 * m11).round().max(1.0) as i32;
        let dest_h = (surface.size.1 as f32 * m22).round().max(1.0) as i32;
        // viewporter takes set_source as wl_fixed_t (24.8 fixed-point);
        // wayland-protocols's WpViewport::set_source converts from f64
        // for us.
        surface.viewport.set_source(
            0.0,
            0.0,
            surface.size.0 as f64,
            surface.size.1 as f64,
        );
        surface.viewport.set_destination(dest_w, dest_h);

        // Subsurface position from translation.
        let tx = transform[4].round() as i32;
        let ty = transform[5].round() as i32;
        surface.wl_subsurface.set_position(tx, ty);

        // Clip via input region.
        match clip {
            Some([x0, y0, x1, y1]) => {
                let region = self
                    .wayland
                    .globals
                    .compositor
                    .create_region(&self.wayland.queue_handle, ());
                region.add(
                    x0.round() as i32,
                    y0.round() as i32,
                    (x1 - x0).round().max(0.0) as i32,
                    (y1 - y0).round().max(0.0) as i32,
                );
                surface.wl_surface.set_input_region(Some(&region));
                region.destroy();
            },
            None => {
                surface.wl_surface.set_input_region(None);
            },
        }

        // Opacity via alpha_modifier (we're in fast path so it's bound).
        if let Some(am) = &surface.alpha_modifier {
            let multiplier =
                (opacity.clamp(0.0, 1.0) * (u32::MAX as f32)).round() as u32;
            am.set_multiplier(multiplier);
        }

        // Attach + damage + commit + flush.
        surface.wl_surface.attach(Some(&slot.wl_buffer), 0, 0);
        surface
            .wl_surface
            .damage_buffer(0, 0, surface.size.0 as i32, surface.size.1 as i32);
        surface.wl_surface.commit();
        self.wayland.flush()?;

        Ok(())
    }

    fn present_baked_path(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) -> Result<(), BackendError> {
        let _ = (key, transform, clip, opacity);
        // Real impl in Phase 7. Stubbed to return Ok for now so the
        // fast path can be exercised without the bake module in flight.
        Err(BackendError::Unwired("present_baked_path — Phase 7"))
    }
}
```

- [ ] **Step 2: Wire `OsCompositorBackend::present`.**

```rust
    fn present(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) {
        if let Err(err) = WaylandSubsurfaceBackend::present_inherent(self, key, transform, clip, opacity) {
            log::warn!("[WaylandSubsurfaceBackend] present({key:?}): {err}");
        }
    }
```

- [ ] **Step 3: Compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend::present — fast path

Per-key per-frame body for the fast path (no rotation, alpha_modifier
protocol available):

1. dispatch_pending to drain release events.
2. Acquire swap-pool slot; roundtrip if starved.
3. copy_texture_to_texture(dest → slot.image); submit on wgpu queue.
4. viewport.set_source(0, 0, w, h) + set_destination scaled from
   transform's m11/m22.
5. subsurface.set_position from translation.
6. clip → set_input_region (Some=create_region+add; None=clear).
7. alpha_modifier.set_multiplier(opacity * u32::MAX).
8. attach + damage_buffer + commit + flush.

Bake-path is stubbed to BackendError::Unwired pending Phase 7.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 6.5: `destroy` + `OsCompositorBackend::sync_mechanism` finalize

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Implement destroy.**

In the `OsCompositorBackend` impl:

```rust
    fn destroy(&mut self, key: SurfaceKey) {
        if let Some(surface) = self.surfaces.remove(&key) {
            surface.wl_subsurface.destroy();
            surface.wl_surface.destroy();
            // viewport + alpha_modifier proxies destroy on drop.
            // swap_pool + bake drop via their own Drop impls.
            drop(surface);
        }
    }
```

- [ ] **Step 2: Confirm `sync_mechanism` + `interop_backend`.**

In the same impl:

```rust
    fn interop_backend(&self) -> InteropBackend {
        InteropBackend::Vulkan
    }

    fn sync_mechanism(&self) -> SyncMechanism {
        SyncMechanism::None
    }
```

- [ ] **Step 3: Compile + clippy.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo clippy -p servo-paint -- -D warnings`
Expected: clean (the `#![allow(dead_code)]` should be removed at this point since everything's wired; remove if clippy doesn't flag a different reason for it).

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend — destroy + finalize trait impl

destroy(key): pops WaylandSurface from surfaces, destroys
wl_subsurface + wl_surface; viewport + alpha_modifier + swap_pool +
bake drop via their own Drop impls.

interop_backend = Vulkan; sync_mechanism = None (same-queue FIFO;
VulkanTimelineSemaphoreSynchronizer dormant on the smoke path).

Backend is now end-to-end without the bake path — fast path covers
scale + translate + opacity-via-alpha-modifier scenes. Phase 7 wires
rotation + alpha-bake fallback.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Bake pipeline (rotation + opacity fallback)

Replaces the stub `BakePipeline` with a real wgpu render pipeline. Lazily allocated bake target per surface. Spec §4.4.

### Task 7.1: WGSL shader + pipeline construction

**Files:**
- Modify: `components/paint/compositor_wayland/bake.rs`

- [ ] **Step 1: Replace the stub with the real pipeline.**

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Rotation + opacity bake pipeline. Used by present() when either
//! (a) the world_transform's linear part has a non-zero off-diagonal
//! (rotation/skew — wp_viewporter can't express it) or
//! (b) wp_alpha_modifier_v1 isn't bound and opacity != 1.0
//! (no per-surface protocol multiplier available).
//!
//! Single textured-quad render pipeline reused across surfaces.

use wgpu::util::DeviceExt;

/// Uniform passed to the bake shader.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct BakeUniform {
    /// Linear 2x2 affine: [m11, m12, m21, m22, _pad, _pad, _pad, _pad].
    /// Padded to 16-byte aligned for WGSL std140 row layout.
    linear: [f32; 4],
    /// Opacity multiplier applied in the fragment shader. 1.0 for
    /// rotation-only bakes.
    opacity: f32,
    _pad: [f32; 3],
}

const SHADER_WGSL: &str = r#"
struct Bake {
    linear: vec4<f32>,
    opacity: f32,
    _pad: vec3<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> bake: Bake;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Quad covering [-1, 1] in source-NDC. Vertex shader applies the
    // linear affine (m11, m12, m21, m22) to rotate/skew the quad into
    // the destination NDC space.
    var positions = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
    );
    let p = positions[vid];
    let m = mat2x2<f32>(
        vec2<f32>(bake.linear.x, bake.linear.y),
        vec2<f32>(bake.linear.z, bake.linear.w),
    );
    let rotated = m * p;
    var out: VsOut;
    out.clip_pos = vec4<f32>(rotated, 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src, src_sampler, in.uv);
    return vec4<f32>(s.rgb, s.a * bake.opacity);
}
"#;

pub struct BakePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
}

impl BakePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("WaylandSubsurfaceBackend bake shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_WGSL.into()),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("WaylandSubsurfaceBackend bake BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("WaylandSubsurfaceBackend bake PL"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("WaylandSubsurfaceBackend bake pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("WaylandSubsurfaceBackend bake sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("WaylandSubsurfaceBackend bake uniform"),
            size: std::mem::size_of::<BakeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
        }
    }

    /// Execute a bake: sample `src` (the per-key dest texture) with the
    /// given linear affine + opacity multiplier, render into `dst` (the
    /// bake-target exportable VkImage wrapped as wgpu::Texture).
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        linear: [f32; 4],
        opacity: f32,
    ) {
        let uniform = BakeUniform {
            linear,
            opacity,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("WaylandSubsurfaceBackend bake BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("WaylandSubsurfaceBackend bake encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("WaylandSubsurfaceBackend bake pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}
```

- [ ] **Step 2: Add `bytemuck` to paint's deps if not already there.**

Check `components/paint/Cargo.toml`. If `bytemuck` isn't already pulled in transitively, add:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
# ... existing ...
bytemuck = { version = "1", features = ["derive"] }
```

(May already be on the workspace; prefer the workspace dep.)

- [ ] **Step 3: Compile.**

Run: `cargo check -p servo-paint`
Expected: success.

- [ ] **Step 4: Commit.**

```bash
git add components/paint/compositor_wayland/bake.rs components/paint/Cargo.toml
git commit -m "$(cat <<'EOF'
paint: bake pipeline — WGSL shader + render pipeline

Single textured-quad render pipeline used by the bake path when
wp_viewporter can't express the transform (rotation/skew) or
wp_alpha_modifier_v1 isn't bound (opacity fallback).

Vertex shader applies the linear 2x2 affine to rotate/skew the quad.
Fragment shader samples the source and multiplies alpha by the
opacity uniform. Single bake() entry point: uploads the uniform,
binds source + sampler + uniform, runs a 4-vertex triangle-strip pass
into the destination.

BakePipeline::new is called once at backend construction; bake() is
called per present() that requires the bake path.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 7.2: Wire bake into `present_baked_path`

**Files:**
- Modify: `components/paint/compositor_wayland/mod.rs`

- [ ] **Step 1: Replace the `present_baked_path` stub.**

```rust
    fn present_baked_path(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) -> Result<(), BackendError> {
        let surface = self.surfaces.get_mut(&key).ok_or_else(|| {
            BackendError::Wayland(format!("present({key:?}): surface not declared"))
        })?;

        // Compute rotated bbox in pixel-space from the source-rect.
        let (src_w, src_h) = surface.size;
        let corners = [
            (0.0_f32, 0.0_f32),
            (src_w as f32, 0.0),
            (0.0, src_h as f32),
            (src_w as f32, src_h as f32),
        ];
        let mapped: Vec<(f32, f32)> = corners
            .iter()
            .map(|(x, y)| {
                (
                    transform[0] * x + transform[2] * y,
                    transform[1] * x + transform[3] * y,
                )
            })
            .collect();
        let min_x = mapped.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let max_x = mapped.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let min_y = mapped.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_y = mapped.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        let bbox_w = (max_x - min_x).ceil().max(1.0) as u32;
        let bbox_h = (max_y - min_y).ceil().max(1.0) as u32;

        // (Re)allocate bake pool on size change.
        let need_realloc = match &surface.bake {
            Some(p) => p.width != bbox_w || p.height != bbox_h,
            None => true,
        };
        if need_realloc {
            let id = surface.surface_id * 10 + 1; // bake-pool subordinate id
            let new_pool = self.allocate_pool(id, bbox_w, bbox_h)?;
            // Re-borrow surface after the &mut self call inside allocate_pool.
            let surface = self.surfaces.get_mut(&key).expect("re-borrowed");
            surface.bake = Some(new_pool);
        }
        // Re-borrow.
        let surface = self.surfaces.get_mut(&key).expect("re-borrowed");
        let bake_pool = surface.bake.as_mut().expect("just allocated");

        let slot_index = loop {
            if let Some(i) = bake_pool.acquire() {
                break i;
            }
            self.wayland.roundtrip()?;
        };
        let slot = &bake_pool.slots[slot_index];

        // Run bake: dest_texture -> slot.image with the linear affine
        // + opacity multiplier (1.0 when alpha_modifier handles opacity).
        let opacity_multiplier = if self.wayland.globals.alpha_modifier.is_some() {
            1.0
        } else {
            opacity
        };
        self.bake_pipeline.bake(
            &self.host.device,
            &self.host.queue,
            &surface.dest_texture,
            &slot.image.wgpu_texture,
            [transform[0], transform[1], transform[2], transform[3]],
            opacity_multiplier,
        );

        // Viewport identity-scales the bbox.
        surface
            .viewport
            .set_source(0.0, 0.0, bbox_w as f64, bbox_h as f64);
        surface.viewport.set_destination(bbox_w as i32, bbox_h as i32);

        // Subsurface position = transform translation + bbox offset.
        let tx = (transform[4] + min_x).round() as i32;
        let ty = (transform[5] + min_y).round() as i32;
        surface.wl_subsurface.set_position(tx, ty);

        // Clip / opacity-via-alpha_modifier mirror the fast path.
        match clip {
            Some([x0, y0, x1, y1]) => {
                let region = self
                    .wayland
                    .globals
                    .compositor
                    .create_region(&self.wayland.queue_handle, ());
                region.add(
                    x0.round() as i32,
                    y0.round() as i32,
                    (x1 - x0).round().max(0.0) as i32,
                    (y1 - y0).round().max(0.0) as i32,
                );
                surface.wl_surface.set_input_region(Some(&region));
                region.destroy();
            },
            None => {
                surface.wl_surface.set_input_region(None);
            },
        }
        if let Some(am) = &surface.alpha_modifier {
            let multiplier =
                (opacity.clamp(0.0, 1.0) * (u32::MAX as f32)).round() as u32;
            am.set_multiplier(multiplier);
        }

        surface.wl_surface.attach(Some(&slot.wl_buffer), 0, 0);
        surface
            .wl_surface
            .damage_buffer(0, 0, bbox_w as i32, bbox_h as i32);
        surface.wl_surface.commit();
        self.wayland.flush()?;

        Ok(())
    }
```

- [ ] **Step 2: Compile + clippy.**

Run: `cargo check -p servo-paint`
Expected: success.

Run: `cargo clippy -p servo-paint -- -D warnings`
Expected: clean.

- [ ] **Step 3: Commit.**

```bash
git add components/paint/compositor_wayland/mod.rs
git commit -m "$(cat <<'EOF'
paint: WaylandSubsurfaceBackend — bake path wired

Replaces the Unwired stub. Computes rotated bbox from the linear
affine; (re)allocates surface.bake on size change; runs the bake
pipeline (dest_texture → slot.image with linear affine + opacity
multiplier — opacity=1.0 when alpha_modifier handles it);
viewport identity-scales; subsurface positions at translation +
bbox offset; clip + alpha_modifier mirror the fast path; attach +
damage + commit + flush.

Triggered when transform[1] or transform[2] is non-zero (rotation/
skew) OR alpha_modifier is None and opacity != 1.0 (protocol fallback).
Smoke scene exercises neither today, but the path is covered for the
moment one or both appear.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Pelt smoke runner + viewer flags

Mirrors the macOS smoke template structurally (winit ApplicationHandler + factory + render loop), forced to Vulkan + Wayland. Spec §9.

### Task 8.1: `smoke_wayland.rs` — config + outcome + non-linux stub

**Files:**
- Create: `ports/pelt-desktop/smoke_wayland.rs`
- Modify: `ports/pelt-desktop/lib.rs`

- [ ] **Step 1: Create the smoke runner stub.**

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Headed presentation smoke on Linux Wayland.
//!
//! Mirrors smoke_macos / smoke_windows in shape: winit window →
//! raw handles → forced wgpu Vulkan backend → netrender Renderer
//! → default_compositor_for_window → render_with_compositor per
//! frame, with optional CompositorSurface declared at 50% opacity
//! for the visual receipt.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub frames_presented: u32,
    pub created_window: bool,
    pub declared_subsurface: bool,
}

#[cfg(feature = "linux-present")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaylandPresentSmokeConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub declare_subsurface: bool,
}

#[cfg(feature = "linux-present")]
impl Default for WaylandPresentSmokeConfig {
    fn default() -> Self {
        Self {
            title: "pelt — wayland-subsurface present smoke".into(),
            width: 800,
            height: 600,
            // ~1s at 60Hz; long enough to confirm the basic smoke is
            // doing real work before auto-exit.
            frames: 60,
            declare_subsurface: false,
        }
    }
}

#[cfg(feature = "linux-present")]
pub fn run_wayland_subsurface_present_smoke(
    config: WaylandPresentSmokeConfig,
) -> Result<WaylandPresentSmokeOutcome, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        return Err("linux-present requires target_os = \"linux\"".into());
    }

    #[cfg(target_os = "linux")]
    {
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = linux_impl::WaylandPresentApp::new(config);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("present-smoke event loop failed: {error}"))?;
        if let Some(error) = app.error {
            return Err(error);
        }
        app.outcome
            .ok_or_else(|| "present smoke ended without an outcome".into())
    }
}

#[cfg(all(feature = "linux-present", target_os = "linux"))]
mod linux_impl {
    use super::*;

    // Real impl lands in 8.2.
    pub struct WaylandPresentApp {
        pub config: WaylandPresentSmokeConfig,
        pub outcome: Option<WaylandPresentSmokeOutcome>,
        pub error: Option<String>,
    }

    impl WaylandPresentApp {
        pub fn new(config: WaylandPresentSmokeConfig) -> Self {
            Self {
                config,
                outcome: None,
                error: None,
            }
        }
    }

    impl winit::application::ApplicationHandler for WaylandPresentApp {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            // Placeholder — Task 8.2.
            let _ = event_loop;
        }
        fn window_event(
            &mut self,
            _: &winit::event_loop::ActiveEventLoop,
            _: winit::window::WindowId,
            _: winit::event::WindowEvent,
        ) {
        }
    }
}
```

- [ ] **Step 2: Wire into `ports/pelt-desktop/lib.rs`.**

Add (in the order mirroring macos/windows):

```rust
#[cfg(feature = "linux-present")]
pub mod smoke_wayland;

#[cfg(feature = "linux-present")]
pub use smoke_wayland::{
    run_wayland_subsurface_present_smoke, WaylandPresentSmokeConfig, WaylandPresentSmokeOutcome,
};
```

- [ ] **Step 3: Compile.**

Run: `cargo check -p pelt-desktop --features linux-present`
Expected: success.

- [ ] **Step 4: Commit.**

```bash
git add ports/pelt-desktop/smoke_wayland.rs ports/pelt-desktop/lib.rs
git commit -m "$(cat <<'EOF'
pelt-desktop: smoke_wayland scaffold + non-linux stub

WaylandPresentSmokeConfig / WaylandPresentSmokeOutcome / dispatch
function shape. linux-impl module is a placeholder App with no-op
handlers; real impl lands next.

Mirrors the smoke_macos shape so cargo check --features linux-present
works on non-Linux hosts (cfg(not(target_os = "linux")) returns
Err("linux-present requires target_os = \"linux\"")).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 8.2: Smoke app impl — winit + Vulkan + netrender + factory + scene

**Files:**
- Modify: `ports/pelt-desktop/smoke_wayland.rs`

- [ ] **Step 1: Replace the placeholder app with the full impl.**

Replace the `linux_impl` module body:

```rust
#[cfg(all(feature = "linux-present", target_os = "linux"))]
mod linux_impl {
    use super::*;

    pub struct WaylandPresentApp {
        pub config: WaylandPresentSmokeConfig,
        window: Option<winit::window::Window>,
        window_id: Option<winit::window::WindowId>,
        state: Option<WaylandPresentState>,
        frames_presented: u32,
        pub outcome: Option<WaylandPresentSmokeOutcome>,
        pub error: Option<String>,
    }

    struct WaylandPresentState {
        renderer: netrender::Renderer,
        compositor: Box<dyn paint::PaintCompositor>,
    }

    impl WaylandPresentApp {
        pub fn new(config: WaylandPresentSmokeConfig) -> Self {
            Self {
                config,
                window: None,
                window_id: None,
                state: None,
                frames_presented: 0,
                outcome: None,
                error: None,
            }
        }

        fn fail(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, message: String) {
            self.error = Some(message);
            event_loop.exit();
        }
    }

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
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pelt wayland-present device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    max_inter_stage_shader_variables: 28,
                    ..Default::default()
                },
                ..Default::default()
            }))
            .map_err(|err| format!("request_device: {err}"))?;
        Ok(netrender::WgpuHandles {
            instance,
            adapter,
            device,
            queue,
        })
    }

    impl winit::application::ApplicationHandler for WaylandPresentApp {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }

            let attributes = winit::window::WindowAttributes::default()
                .with_title(self.config.title.clone())
                .with_inner_size(winit::dpi::LogicalSize::new(
                    self.config.width as f64,
                    self.config.height as f64,
                ));
            let window = match event_loop.create_window(attributes) {
                Ok(w) => w,
                Err(err) => return self.fail(event_loop, format!("create_window: {err}")),
            };
            self.window_id = Some(window.id());

            use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
            let display_handle = match window.display_handle() {
                Ok(h) => h.as_raw(),
                Err(err) => return self.fail(event_loop, format!("display_handle: {err}")),
            };
            let window_handle = match window.window_handle() {
                Ok(h) => h.as_raw(),
                Err(err) => return self.fail(event_loop, format!("window_handle: {err}")),
            };

            let handles = match build_vulkan_handles() {
                Ok(h) => h,
                Err(err) => return self.fail(event_loop, format!("wgpu Vulkan boot: {err}")),
            };
            let device = handles.device.clone();
            let queue = handles.queue.clone();
            let renderer = match netrender::create_netrender_instance(
                handles,
                netrender::NetrenderOptions {
                    tile_cache_size: Some(64),
                    enable_vello: true,
                    ..Default::default()
                },
            ) {
                Ok(r) => r,
                Err(err) => {
                    return self
                        .fail(event_loop, format!("create_netrender_instance: {err:?}"));
                },
            };

            let host = paint::HostWgpuContext::new(device, queue);
            let compositor =
                match paint::default_compositor_for_window(host, display_handle, window_handle) {
                    Ok(c) => c,
                    Err(err) => {
                        return self
                            .fail(event_loop, format!("default_compositor_for_window: {err}"));
                    },
                };

            self.state = Some(WaylandPresentState {
                renderer,
                compositor,
            });

            window.request_redraw();
            self.window = Some(window);
        }

        fn window_event(
            &mut self,
            event_loop: &winit::event_loop::ActiveEventLoop,
            window_id: winit::window::WindowId,
            event: winit::event::WindowEvent,
        ) {
            if Some(window_id) != self.window_id {
                return;
            }

            match event {
                winit::event::WindowEvent::CloseRequested => event_loop.exit(),
                winit::event::WindowEvent::Resized(_) => {
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                },
                winit::event::WindowEvent::RedrawRequested => {
                    let Some(state) = self.state.as_mut() else {
                        return;
                    };
                    let Some(window) = self.window.as_ref() else {
                        return;
                    };

                    if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                        return;
                    }

                    let inner = window.inner_size();
                    let backing_w = inner.width.max(1);
                    let backing_h = inner.height.max(1);
                    let mut scene = netrender::Scene::new(backing_w, backing_h);
                    // Background red over the full viewport.
                    scene.push_rect(
                        0.0,
                        0.0,
                        backing_w as f32,
                        backing_h as f32,
                        [1.0, 0.0, 0.0, 1.0],
                    );
                    if self.config.declare_subsurface {
                        // Top-left quarter green; per-surface composes at 50% opacity.
                        let half_w = backing_w as f32 / 2.0;
                        let half_h = backing_h as f32 / 2.0;
                        scene.push_rect(0.0, 0.0, half_w, half_h, [0.0, 1.0, 0.0, 1.0]);
                        let mut surface = netrender::CompositorSurface::new(
                            netrender::SurfaceKey(1),
                            [0.0, 0.0, half_w, half_h],
                        );
                        surface.opacity = 0.5;
                        scene.declare_compositor_surface(surface);
                    }

                    let pc: &mut dyn paint::PaintCompositor = &mut *state.compositor;
                    state.renderer.render_with_compositor(
                        &scene,
                        wgpu::TextureFormat::Rgba8Unorm,
                        pc,
                        netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
                    );

                    self.frames_presented += 1;

                    if self.config.frames > 0 && self.frames_presented >= self.config.frames {
                        event_loop.exit();
                    } else if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                },
                _ => {},
            }
        }

        fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
            if self.outcome.is_some() {
                return;
            }
            self.outcome = Some(WaylandPresentSmokeOutcome {
                width: self.config.width,
                height: self.config.height,
                frames_presented: self.frames_presented,
                created_window: self.window_id.is_some(),
                declared_subsurface: self.config.declare_subsurface,
            });
        }
    }
}
```

- [ ] **Step 2: Compile.**

Run: `cargo check -p pelt-desktop --features linux-present`
Expected: success.

- [ ] **Step 3: Commit.**

```bash
git add ports/pelt-desktop/smoke_wayland.rs
git commit -m "$(cat <<'EOF'
pelt-desktop: smoke_wayland — full impl (winit + Vulkan + factory)

WaylandPresentApp implements winit::application::ApplicationHandler:
- resumed: create winit window, pull raw handles, force wgpu Vulkan
  via build_vulkan_handles, create netrender Renderer, build
  HostWgpuContext, hand to default_compositor_for_window which dispatches
  to WaylandSubsurfaceBackend on Linux.
- RedrawRequested: build netrender::Scene(red full viewport; optional
  green top-left quarter + CompositorSurface at 50% opacity); call
  render_with_compositor; auto-exit at frames>=cap or request_redraw.
- exiting: finalize outcome.

`frames: 0` keeps the window open until user-close — the visual
receipt path for --wayland-present-surfaces-smoke.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 8.3: Viewer flags + dispatch + help text

**Files:**
- Modify: `ports/pelt/viewer.rs`

- [ ] **Step 1: Add the flag parsing variables.**

After the existing `#[cfg(feature = "macos-present")] let mut macos_present_surfaces_smoke = false;`:

```rust
    #[cfg(feature = "linux-present")]
    let mut wayland_present_smoke = false;
    #[cfg(feature = "linux-present")]
    let mut wayland_present_surfaces_smoke = false;
```

- [ ] **Step 2: Add the arg-match arms.**

After the macOS arms:

```rust
            #[cfg(feature = "linux-present")]
            "--wayland-present-smoke" => {
                wayland_present_smoke = true;
            },
            #[cfg(feature = "linux-present")]
            "--wayland-present-surfaces-smoke" => {
                wayland_present_surfaces_smoke = true;
            },
```

- [ ] **Step 3: Add the dispatch.**

After the macOS dispatch:

```rust
    #[cfg(feature = "linux-present")]
    if wayland_present_smoke {
        run_optional_wayland_present_smoke();
        return;
    }

    #[cfg(feature = "linux-present")]
    if wayland_present_surfaces_smoke {
        run_optional_wayland_present_surfaces_smoke();
        return;
    }
```

- [ ] **Step 4: Add the runner functions.**

After `run_optional_macos_present_surfaces_smoke`:

```rust
#[cfg(feature = "linux-present")]
fn run_optional_wayland_present_smoke() {
    let config = pelt_desktop::WaylandPresentSmokeConfig::default();
    match pelt_desktop::run_wayland_subsurface_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt wayland-present smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "linux-present")]
fn run_optional_wayland_present_surfaces_smoke() {
    // Same shape as the basic smoke but flips `declare_subsurface`
    // on and runs frames=0 (held until window close) so the per-
    // surface composition is visible long enough for the visual
    // receipt: red master + green declared-quarter at 50% opacity
    // producing olive blend where they compose.
    let config = pelt_desktop::WaylandPresentSmokeConfig {
        title: "pelt — wayland-subsurface present smoke (with declared surface)".into(),
        declare_subsurface: true,
        frames: 0,
        ..pelt_desktop::WaylandPresentSmokeConfig::default()
    };
    match pelt_desktop::run_wayland_subsurface_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt wayland-present surfaces smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}
```

- [ ] **Step 5: Add the help text.**

In `print_help`, after the macos-present lines:

```
    --wayland-present-smoke            (requires --features linux-present, target_os = "linux")
    --wayland-present-surfaces-smoke   (same as --wayland-present-smoke + a declared compositor surface)
```

- [ ] **Step 6: Compile.**

Run: `cargo build -p pelt --features linux-present`
Expected: success.

Run: `cargo clippy -p pelt --features linux-present -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit.**

```bash
git add ports/pelt/viewer.rs
git commit -m "$(cat <<'EOF'
pelt: --wayland-present-{,surfaces-}smoke + help

Adds the two Wayland smoke flags behind cfg(feature = "linux-present"),
parallel to the windows-present / macos-present arms.

Basic smoke uses the default config (frames=60 auto-exit). Surfaces
smoke declares one CompositorSurface at 50% opacity over the top-left
quarter and runs frames=0 (held until user-close) for the visual
receipt — red master + green declared-quarter + olive blend.

Help text + dispatch wired in the same shape as the other platforms.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9 — Validation runs

No commits in this phase. Each step is a verification gate; if any fails, return to the relevant earlier phase, fix, then re-run.

### Task 9.1: Build + clippy + cargo-tree gating

- [ ] **Step 1:** `cargo build -p pelt --features linux-present` → exit 0.
- [ ] **Step 2:** `cargo clippy -p servo-paint -- -D warnings` → clean.
- [ ] **Step 3:** `cargo clippy -p pelt-desktop --features linux-present -- -D warnings` → clean.
- [ ] **Step 4:** `cargo test -p servo-paint --lib interop::vulkan_timeline -- --nocapture` → both tests PASS.
- [ ] **Step 5:** `cargo test -p servo-paint --lib compositor_wayland::dmabuf::tests -- --nocapture` → all PASS.
- [ ] **Step 6:** `cargo tree -p pelt --target x86_64-pc-windows-msvc 2>&1 | grep -E '^(\| )*(ash|wayland-client|wayland-protocols)' || echo OK_no_linux_deps_on_windows` → `OK_no_linux_deps_on_windows`.

### Task 9.2: Headless basic smoke

- [ ] **Step 1:** `cargo run -p pelt --features linux-present -- --wayland-present-smoke`
- [ ] **Step 2:** Verify: exits 0 within ~1s.
- [ ] **Step 3:** Verify stdout matches: `pelt wayland-present smoke 800x600 frames=60 created_window=true declared_subsurface=false`.
- [ ] **Step 4:** Verify backend log line printed during the run: `[WaylandSubsurfaceBackend] dmabuf modifier: format=0x34324241 modifier=0x0000000000000000` (`0x34324241` is `"AB24"` little-endian = `DRM_FORMAT_ABGR8888`; modifier `0` is `DRM_FORMAT_MOD_LINEAR`).

If any of the above fails: capture `RUST_LOG=debug WAYLAND_DEBUG=1 cargo run -p pelt --features linux-present -- --wayland-present-smoke 2>&1 | head -120` and diagnose. Common likely issues: dmabuf modifier negotiation surfacing no LINEAR (Phase 4.3 picker), VkImage allocation rejecting the modifier (RADV-specific format-support check in Phase 4.2), wayland-client API drift (Phase 5.1 adopt_surface).

### Task 9.3: Visual receipt + surfaces smoke

- [ ] **Step 1:** `cargo run -p pelt --features linux-present -- --wayland-present-surfaces-smoke`
- [ ] **Step 2:** A window titled "pelt — wayland-subsurface present smoke (with declared surface)" should open at 800×600.
- [ ] **Step 3:** Visually verify:
  - Red fills the bottom-right ¾ of the window.
  - Top-left ¼ shows **olive/yellow** (master red × 0.5 + per-surface green × 0.5 = olive blend) — *not* pure green and *not* pure red.
  - No flicker on resize.
- [ ] **Step 4:** Close the window. Process exits 0.
- [ ] **Step 5:** Verify stdout: `pelt wayland-present surfaces smoke 800x600 frames={N} created_window=true declared_subsurface=true` for some N > 0.

If the visual receipt shows pure green (no blend): the alpha-modifier branch may not be wired; check `cargo run` logs for `[WaylandSubsurfaceBackend]` advertising `alpha_modifier=true` (debug log from `wayland::WaylandState::new`).

If pure red (no per-surface visible): the per-key subsurface may not be attaching/committing; run with `WAYLAND_DEBUG=1` and confirm `wl_surface@N.attach(wl_buffer@M, 0, 0)` + `commit` per frame for surface N matching the declared one.

### Task 9.4: Per-frame Wayland protocol trace (sanity)

- [ ] **Step 1:** `WAYLAND_DEBUG=1 cargo run -p pelt --features linux-present -- --wayland-present-surfaces-smoke 2>&1 | head -200`
- [ ] **Step 2:** Confirm presence (any order):
  - `wl_compositor@N.create_surface` (×2 — parent already exists, per-key)
  - `wl_subcompositor@N.get_subsurface`
  - `wp_viewporter@N.get_viewport`
  - `wp_alpha_modifier_v1@N.get_surface`
  - `zwp_linux_dmabuf_v1@N.create_params` (×N pool slots: 2 for master + 2 for per-key = 4 min)
  - `zwp_linux_buffer_params_v1@N.add`
  - `zwp_linux_buffer_params_v1@N.create_immed`
  - Per frame: `wl_surface@N.attach(wl_buffer@M, 0, 0)` + `wl_surface@N.damage_buffer(...)` + `wl_surface@N.commit()` for both master and per-key
  - `wl_buffer@M.release` events streaming from Mutter at ~60Hz

---

## Phase 10 — Documentation updates

Flip the four landing docs to reflect the C4 universally-green state on Linux. Spec §11.

### Task 10.1: Flip C4 gap (4) to ✅

**Files:**
- Modify: `docs/2026-05-09_c4_landed_notes.md`

- [ ] **Step 1:** Find the gap (4) entry:

```
4. **Linux smoke receipt.** `WaylandSubsurfaceBackend` is still a
   skeleton — `wl_subsurface` placement + commit, `dmabuf` import
   path need a Wayland session (Mutter or Sway) to validate.
```

Replace with the ✅-style entry, mirroring the macOS gap (1) prose:

```
4. **Linux smoke receipt — ✅ landed (2026-06-03).** `WaylandSubsurfaceBackend::new`
   now constructs end-to-end: borrows the embedder's `wl_display` + `wl_surface`
   via `wayland_client::Connection::from_ptr`, binds the required globals
   (`wl_compositor` v4-6, `wl_subcompositor`, `zwp_linux_dmabuf_v1` v3-4,
   `wp_viewporter`, optional `wp_alpha_modifier_v1`), runs an initial roundtrip
   to drain dmabuf format/modifier advertisements, and picks the
   `(DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR)` pair. `present_master` blits
   the netrender master into a per-frame side-buffer (dmabuf-exportable VkImage
   wrapped as `wgpu::Texture` via `wgpu::hal::vulkan::Device::texture_from_raw` +
   `create_texture_from_hal::<Vulkan>`) and attaches the resulting `wl_buffer`
   to the parent `wl_surface` via `zwp_linux_dmabuf_v1.create_params` →
   `params.create_immed`. The per-`SurfaceKey` `declare`/`destroy`/`present`
   paths are wired: `declare` creates a per-key `wl_surface` + `wl_subsurface`
   (parented to the embedder surface, `set_desync`), a `wp_viewport` for
   transform/destination, and (if advertised) a `wp_alpha_modifier_surface_v1`;
   `present` copies the per-key destination texture into a two-slot mailbox
   dmabuf pool (recycled via `wl_buffer.release` events), applies viewport +
   input region + alpha-modifier, then attach/damage/commit/flush. A bake
   render pass (rotation + opacity fallback) covers transforms `wp_viewporter`
   can't express. Visual receipt: `pelt --wayland-present-surfaces-smoke` shows
   the per-surface composite correctly at 50% opacity over the master master
   (olive blend where master red shows through, pure green where master green
   is occluded). The `VulkanTimelineSemaphoreSynchronizer` interop slot is filled
   (idiomatic Vulkan-timeline shape: semaphore handle + `next_value` +
   `signaled_value` via `vkGetSemaphoreCounterValue` + `wait_host` via
   `vkWaitSemaphores` + OPAQUE_FD export); dormant on the smoke path because
   same-queue FIFO covers it, ready for cross-queue / cross-process consumers.
```

- [ ] **Step 2:** Find the summary line at the bottom of "Remaining gaps":

```
(0) is ✅ ... (1) is ✅. (2) is ✅ as well. (3) was a wait-and-see ... (4) gates D3 ✅ on Linux. (5) is ✅. Once (0) lands, the remaining roadmap should move to C5/C7 while Linux stays externally gated.
```

Replace with the universally-green summary:

```
(0) is ✅. (1) is ✅. (2) is ✅ as well. (3) was a wait-and-see deferred to (2)'s landing — the master-path sync is now FIFO-ordered without the wgpu-hal queue accessor; future per-`SurfaceKey` GPU sync upgrades may still want it, but no longer block anything visible. (4) is ✅ as of 2026-06-03 — Linux Wayland smoke + visual receipt on Fedora 44 / GNOME-Mutter. (5) is ✅. C4 is universally green; the roadmap can move to C5/C7 without Linux gating.
```

- [ ] **Step 2 commit:** keep for the docs commit in Task 10.4.

### Task 10.2: Flip interop lineage Linux slot to ✓

**Files:**
- Modify: `docs/2026-05-09_interop_lineage.md`

- [ ] **Step 1:** Find the "Pending: Mac and Linux synchronizer wrappers" section.

Update the Linux bullet from pending to landed:

```
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
```

- [ ] **Step 2:** Section title "Pending: Mac and Linux synchronizer wrappers" → "Pending: Mac synchronizer wrapper" (only one remains).

### Task 10.3: Brief annotation + cut plan update

**Files:**
- Modify: `docs/2026-05-28_wayland_per_surface_presentation_gap.md`
- Modify: `docs/archive/2026-05-05_genet_netrender_cut_plan.md`

- [ ] **Step 1:** Append a "Done" section to the wayland brief:

```
---

## Done — 2026-06-03

Landed on the Fedora 44 (GNOME / Mutter, RADV / Mesa 26.0.6) validation
box. Headless `--wayland-present-smoke` exits 0; visual receipt via
`--wayland-present-surfaces-smoke` shows the red master + olive-blended
per-surface green-quarter at 50% opacity.

See [`docs/superpowers/specs/2026-06-03-wayland-per-surface-presentation-design.md`](./superpowers/specs/2026-06-03-wayland-per-surface-presentation-design.md)
for the implementation design; [`docs/2026-05-09_c4_landed_notes.md`](./2026-05-09_c4_landed_notes.md)
gap (4) entry for the landed-state summary; [`docs/2026-05-09_interop_lineage.md`](./2026-05-09_interop_lineage.md)
for the `VulkanTimelineSemaphoreSynchronizer` slot fill.
```

- [ ] **Step 2:** In the cut plan, find the C4 status snapshot (most likely in a "C4" section or table). Update the Linux row from "pending" / "Linux pending an on-device session" to "landed 2026-06-03; see `docs/2026-05-09_c4_landed_notes.md` gap (4)." Update the C4 done-condition status to "universally green."

### Task 10.4: Commit docs

- [ ] **Step 1:** Stage all four edited docs.

```bash
git add docs/2026-05-09_c4_landed_notes.md docs/2026-05-09_interop_lineage.md docs/2026-05-28_wayland_per_surface_presentation_gap.md docs/archive/2026-05-05_genet_netrender_cut_plan.md
git commit -m "$(cat <<'EOF'
docs: c4 — flip Linux smoke receipt to ✅; interop lineage Linux slot ✓

C4 is now universally green. Updates:

- 2026-05-09_c4_landed_notes.md gap (4) flipped from "skeleton" to ✅
  with the landed-state summary (globals binding, dmabuf VkImage
  export, mailbox pool, viewporter + alpha-modifier, bake fallback)
  and the visual-receipt confirmation.
- 2026-05-09_interop_lineage.md: Linux slot moved from "Pending" to
  landed, describing the idiomatic Vulkan-timeline shape (semaphore
  handle + next_value + signaled_value + wait_host + OPAQUE_FD
  export — no empty-buffer signal/wait submits).
- 2026-05-28_wayland_per_surface_presentation_gap.md: append a
  Done-2026-06-03 section linking the implementation design spec,
  C4 notes, and lineage slot.
- archive/2026-05-05_genet_netrender_cut_plan.md C4 row updated.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Self-review

Quick pass against the spec:

- §1 (goal) → covered by Phases 6-9 (backend impl + smoke + receipts).
- §2 (out-of-scope) → respected; no `wp_linux_drm_syncobj_v1`, no X11, no rotation in smoke scene.
- §3 (current state) → Task 3.1 + 6.1 carry the skeleton-to-real transition.
- §4.1 (topology) → Tasks 6.2 (master on parent) + 6.3 (per-key on subsurface, set_desync).
- §4.2 (implicit sync) → Phase 4.4 (release-event wiring); `sync_mechanism = None` in Task 6.5.
- §4.3 (exportable VkImage) → Task 4.2.
- §4.4 (bake path) → Phase 7.
- §4.5 (synchronizer slot) → Phase 2.
- §5 (module structure + gating) → Phase 1 (deps), Task 3.1 (module dir).
- §6 (Item 1 backend) → Phase 6.
- §7 (Item 2 dmabuf) → Phase 4.
- §8 (Item 3 synchronizer) → Phase 2.
- §9 (Item 4 smoke) → Phase 8.
- §10 (validation) → Phase 9.
- §11 (docs) → Phase 10.
- §12 (risks) → Phase 9.2's "if any of the above fails" diagnostic guidance addresses each named risk.
- §13 (implementation order) → Phases 1→10 match the suggested order with minor compactions (errors.rs folded into the module-dir promotion task; bake stub-then-real folded into Phases 6→7 so the backend can compile incrementally).

Placeholder scan: none. Every step has actual code, expected output, or a concrete verification.

Type consistency: `WaylandSurface` struct revised in Task 6.3 carries forward to 6.4/6.5/7.2 with `dest_texture` + `swap_pool` + `bake` field names matching. `SurfaceBufferPool` field names (`slots`, `width`, `height`, `chosen`) consistent across 4.4 / 6.2 / 6.3. `ExportableImage::wgpu_texture` accessed in 6.2 (master), 6.3 (declare), 6.4 (present). `VulkanTimelineSemaphoreSynchronizer` method names match across 2.1-2.4 and 10.2 (docs).

One known API-drift risk (flagged inline): wayland-client 0.31's exact `from_foreign_display` / `ObjectId::from_ptr` calling convention. The plan documents the intent; the implementer follows the compiler's diagnostic if the named-function changes minorly.
