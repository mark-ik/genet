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
    /// Real impl lands in Task 4.2.
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
