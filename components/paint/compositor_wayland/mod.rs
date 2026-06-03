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
//! Substantive infrastructure landed; backend skeleton remains. Phase 4
//! (dmabuf): `ExportableImage` allocates VkImages with the dmabuf+modifier
//! chain, exports the fd, and wraps the VkImage back into a `wgpu::Texture`;
//! `ModifierTable` picks `(ABGR8888, LINEAR)` from the Vulkan/Wayland
//! intersection; `SurfaceBufferPool` is the N=2 mailbox pool keyed by
//! `BufferSlotUserData`. Phase 5 (wayland): `WaylandState` adopts the
//! embedder's `wl_display`/`wl_surface`, binds globals, dispatches release
//! events.
//!
//! The `WaylandSubsurfaceBackend` struct itself still has the skeleton
//! `present_master` / `declare` / `present` / `destroy` paths returning
//! `BackendError::Unwired` (or trait defaults). Phase 6 replaces those with
//! the per-frame body that uses the infrastructure above.

#![allow(unsafe_code)]
#![allow(dead_code)]

mod bake;
mod dmabuf;
mod errors;
mod wayland;

pub use errors::BackendError;

use std::ffi::c_void;
use std::os::fd::IntoRawFd;
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

impl OsCompositorBackend for WaylandSubsurfaceBackend {
    fn interop_backend(&self) -> InteropBackend {
        InteropBackend::Vulkan
    }

    fn sync_mechanism(&self) -> SyncMechanism {
        // Same-queue submits are FIFO-ordered on Vulkan; the
        // VulkanTimelineSemaphoreSynchronizer wrapper exists for the
        // multi-queue path but is dormant on the smoke path.
        SyncMechanism::None
    }

    fn present_master(&mut self, _master: &Texture) {
        // Real impl lands in Task 6.2.
        log::warn!("[WaylandSubsurfaceBackend] present_master: unwired (Task 6.2)");
    }

    // declare / destroy / present inherit the trait defaults (no-ops
    // for now) until Tasks 6.3-6.5 wire them.
}
