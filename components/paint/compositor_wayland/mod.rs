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
use std::os::fd::{AsFd, IntoRawFd};
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

impl WaylandSubsurfaceBackend {
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
        // Dup the fd so the wayland-side close doesn't disturb the
        // Vulkan-side memory. Pass a BorrowedFd view; wayland-client
        // will dup it again when serialising the message.
        let dup_fd = image
            .dmabuf_fd
            .try_clone()
            .map_err(|e| BackendError::Dmabuf(format!("dup fd: {e}")))?;
        params.add(
            dup_fd.as_fd(),
            0,                               // plane_idx
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

    fn present_master(&mut self, master: &Texture) {
        if let Err(err) = WaylandSubsurfaceBackend::present_master(self, master) {
            log::warn!("[WaylandSubsurfaceBackend] present_master: {err}");
        }
    }

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

    // destroy / present inherit the trait defaults (no-ops
    // for now) until Tasks 6.4-6.5 wire them.
}
