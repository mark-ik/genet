/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Linux Wayland subsurface `OsCompositorBackend` impl.
//!
//! Bridges netrender's wgpu Vulkan master texture into a
//! `wl_subsurface` attached to the embedder's `wl_surface`. Per-frame
//! flow exports the master as a DMABUF (via the
//! `VK_EXT_external_memory_dma_buf` Vulkan extension), wraps it in a
//! `wl_buffer` via the `linux-dmabuf-v1` Wayland protocol, attaches
//! the buffer to the subsurface, and commits the surface tree.
//!
//! ## Construction inputs
//!
//! The backend takes raw pointers to `wl_display` and `wl_surface`
//! (the embedder's window surface). raw-window-handle's
//! `WaylandDisplayHandle` and `WaylandWindowHandle` are the canonical
//! source — the pelt embedder pulls them from winit and hands the
//! `display` / `surface` fields here.
//!
//! ## Why this is dep-light right now
//!
//! A working Wayland backend needs `wayland-client` + the
//! `linux-dmabuf-v1` protocol from `wayland-protocols`, plus
//! `ash`-shaped Vulkan-to-DMABUF export glue. That's ~400-600 LOC of
//! protocol + extension code that needs to be authored on a Linux
//! workstation where `cargo check` actually runs the wayland
//! protocol-binding macros against a live `libwayland-client.so`.
//!
//! This file is the **shape lock**: trait signature, construction
//! argument types, FIXME-documented per-frame steps. The full impl
//! lands in a focused Linux session that adds the deps and authors
//! the protocol code.
//!
//! ## Status
//!
//! **Skeleton.** Does not compile any wayland-client code. Returns
//! [`BackendError::Unwired`] from every meaningful operation.

#![allow(unsafe_code)]
#![allow(dead_code)]

mod errors;

pub use errors::BackendError;

use rustc_hash::FxHashMap;
use wgpu::Texture;

use crate::compositor::OsCompositorBackend;
use crate::interop::{HostWgpuContext, InteropBackend, SyncMechanism};
use netrender_device::compositor::SurfaceKey;

/// Linux Wayland subsurface compositor backend.
///
/// Holds raw pointers to the embedder's wl_display + wl_surface; the
/// caller is responsible for keeping the underlying Wayland objects
/// alive for the backend's lifetime.
pub struct WaylandSubsurfaceBackend {
    /// `*mut wl_display`. Pulled from
    /// raw-window-handle::WaylandDisplayHandle.
    display: *mut std::ffi::c_void,
    /// `*mut wl_surface` (the embedder's window surface). Pulled
    /// from raw-window-handle::WaylandWindowHandle.
    parent_surface: *mut std::ffi::c_void,
    /// FIXME(C4 follow-up): once wayland-client is wired, hold:
    /// - `wl_compositor`, `wl_subcompositor`, `wl_subsurface`
    /// - `zwp_linux_dmabuf_v1` global
    /// - per-`SurfaceKey` `wl_subsurface` + `wl_buffer` map
    surfaces: FxHashMap<SurfaceKey, WaylandSurface>,
    /// Vulkan export-fence value the producer signals at after
    /// netrender's submit completes. Reserved for the multi-queue
    /// path; same-queue submits are FIFO-ordered without explicit
    /// waits.
    next_export_value: std::cell::Cell<u64>,
}

struct WaylandSurface {
    /// `*mut wl_subsurface`.
    subsurface: *mut std::ffi::c_void,
    /// Last `wl_buffer` attached to the subsurface; replaced on
    /// each `present`.
    last_buffer: Option<*mut std::ffi::c_void>,
}

unsafe impl Send for WaylandSubsurfaceBackend {}

impl WaylandSubsurfaceBackend {
    /// Construct a backend over the embedder's wayland display +
    /// surface. Both must be non-null and outlive the backend.
    ///
    /// # Safety
    ///
    /// `display` must point to a valid `wl_display`; `parent_surface`
    /// to a valid `wl_surface`. Both ownerships stay with the
    /// caller; the backend only borrows.
    pub unsafe fn new(
        host: &HostWgpuContext,
        display: *mut std::ffi::c_void,
        parent_surface: *mut std::ffi::c_void,
    ) -> Result<Self, BackendError> {
        if host.backend != InteropBackend::Vulkan {
            return Err(BackendError::WrongBackend(host.backend));
        }
        if display.is_null() {
            return Err(BackendError::NullDisplay);
        }
        if parent_surface.is_null() {
            return Err(BackendError::NullSurface);
        }

        // FIXME(C4 follow-up): with wayland-client added:
        //   1. `Connection::from_ptr(display)`
        //   2. `globals::registry_queue_init` to get
        //      `wl_compositor`, `wl_subcompositor`,
        //      `zwp_linux_dmabuf_v1`
        //   3. capability-check that the compositor advertises the
        //      DRM format `ARGB8888` (or `XRGB8888`) on a renderable
        //      modifier
        //
        // For now: just stash the pointers.
        let _ = host; // FIXME wire wgpu-hal Vulkan device for DMABUF export

        Ok(Self {
            display,
            parent_surface,
            surfaces: FxHashMap::default(),
            next_export_value: std::cell::Cell::new(0),
        })
    }

    /// Present the netrender master texture as a wayland subsurface
    /// on the parent.
    ///
    /// FIXME(C4 follow-up): the per-frame body needs:
    /// 1. Pull master's underlying VkImage via
    ///    `master.as_hal::<Vulkan>().raw_handle()`.
    /// 2. Export the VkImage as a DMABUF via
    ///    `vkGetMemoryFdKHR(VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT)`.
    ///    Need an `ash::Device` extension reference.
    /// 3. `zwp_linux_buffer_params_v1.create` from the dmabuf fd,
    ///    awaiting the `created` event for the resulting `wl_buffer`.
    /// 4. `wl_subsurface.set_position(0, 0)`,
    ///    `wl_surface.attach(buffer, 0, 0)`,
    ///    `wl_surface.damage_buffer(0, 0, w, h)`,
    ///    `wl_surface.commit()`.
    /// 5. `wl_display.flush()` to push the protocol writes.
    ///
    /// Today: returns `Unwired`. The signature locks in the trait
    /// surface for the per-platform impl.
    pub fn present_master(&mut self, _master: &Texture) -> Result<(), BackendError> {
        let v = self.next_export_value.get();
        self.next_export_value.set(v + 1);
        Err(BackendError::Unwired("present_master per-frame body"))
    }
}

impl OsCompositorBackend for WaylandSubsurfaceBackend {
    fn interop_backend(&self) -> InteropBackend {
        InteropBackend::Vulkan
    }

    fn sync_mechanism(&self) -> SyncMechanism {
        // Same-queue submits are FIFO-ordered on Vulkan; the
        // explicit external-semaphore path is reserved for the
        // multi-queue future.
        SyncMechanism::None
    }

    fn present_master(&mut self, master: &Texture) {
        if let Err(err) = WaylandSubsurfaceBackend::present_master(self, master) {
            log::warn!("[WaylandSubsurfaceBackend] present_master: {err}");
        }
    }

    // `declare` and `present` inherit the trait defaults until the
    // per-`SurfaceKey` Wayland path is wired (create a
    // `wl_subsurface` for `key` with sync-mode Desync; on present
    // apply transform via viewporter `wp_viewport.set_destination`,
    // clip via the surface's input region, opacity via the
    // alpha-modifier protocol if available). The default `declare`
    // does plain wgpu allocation, which is the right starting
    // point for any backend that hasn't yet wired its per-surface
    // OS handoff.

    fn destroy(&mut self, key: SurfaceKey) {
        // Mirrors the per-subsurface cleanup that lands when
        // `declare` is wired (destroy `wl_subsurface`, release any
        // attached `wl_buffer`). Today `surfaces` is never
        // populated (declare uses the default plain-alloc path), so
        // this is a no-op in practice — kept so future per-surface
        // impl doesn't need to redefine cleanup.
        self.surfaces.remove(&key);
    }
}
