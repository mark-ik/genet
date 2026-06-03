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
use std::sync::{Arc, Mutex};

use ash::vk;
use smallvec::SmallVec;
use wayland_client::protocol::wl_buffer::WlBuffer;

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
                .extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
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
            .ok_or_else(|| {
                vk_device.destroy_image(vk_image, None);
                BackendError::Dmabuf("no compatible memory type".into())
            })?;

            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(vk_image);
            let mut export_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let alloc_info = vk::MemoryAllocateInfo::default()
                .allocation_size(mem_req.size)
                .memory_type_index(mem_type_index)
                .push_next(&mut dedicated)
                .push_next(&mut export_info);

            let vk_memory = vk_device.allocate_memory(&alloc_info, None).map_err(|e| {
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
            let raw_fd = external_memory_fd.get_memory_fd(&get_fd_info).map_err(|e| {
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
                    vk_device.free_memory(vk_memory, None);
                    vk_device.destroy_image(vk_image, None);
                    BackendError::Dmabuf(format!(
                        "get_image_drm_format_modifier_properties: {e}"
                    ))
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
        let wgpu_texture =
            wrap_vk_image_as_wgpu(host, &vk_device, vk_image, vk_memory, width, height)?;

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
    // Drop callback: wgpu invokes this when the wgpu::Texture's last
    // ref drops. Destroys the image, frees the memory.
    let device_for_drop = vk_device.clone();
    let drop_image = vk_image;
    let drop_memory = vk_memory;
    let drop_callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
        device_for_drop.destroy_image(drop_image, None);
        device_for_drop.free_memory(drop_memory, None);
    });

    let hal_descriptor = wgpu::hal::TextureDescriptor {
        label: Some("ExportableImage dmabuf"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
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

    let wgpu_texture = unsafe {
        let hal_device = host
            .device
            .as_hal::<wgpu::wgc::api::Vulkan>()
            .ok_or_else(|| BackendError::Dmabuf("wgpu-hal Vulkan device unavailable".into()))?;
        let hal_texture = hal_device.texture_from_raw(
            vk_image,
            &hal_descriptor,
            Some(drop_callback),
            wgpu::hal::vulkan::TextureMemory::External,
        );
        drop(hal_device);

        host.device.create_texture_from_hal::<wgpu::wgc::api::Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("ExportableImage dmabuf"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
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
    /// LINEAR after verifying Vulkan can import it. Trusts that the
    /// compositor accepts LINEAR (dmabuf-protocol baseline).
    pub fn choose(&self) -> Result<ChosenModifier, BackendError> {
        let vk_linear = self.vulkan_importable.contains(&DRM_FORMAT_MOD_LINEAR);
        if !vk_linear {
            return Err(BackendError::NoCompatibleFormat);
        }
        // v1 LINEAR-only picker: trust that the compositor accepts LINEAR
        // (the dmabuf-protocol baseline). The Wayland-side advertisement
        // check is intentionally soft because Mutter uses v4 feedback
        // (format_table + tranche_formats) and our Dispatch impl only
        // collects v3 Modifier events — leaving `advertised` empty even
        // when the compositor accepts every modifier. Real v4 feedback
        // parsing is a Phase-7-style follow-up; for now we rely on the
        // create_immed call failing visibly if Mutter ever rejects
        // LINEAR (extremely unlikely on Mesa).
        log::info!(
            "[ModifierTable] vk_importable={} wayland_advertised={} entries; \
             v1 picker hard-codes LINEAR",
            self.vulkan_importable.len(),
            self.advertised.len(),
        );
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
    fn choose_succeeds_with_only_vulkan_linear_even_if_wayland_has_nothing() {
        // v1 picker hard-codes LINEAR after verifying Vulkan; trusts
        // the compositor accepts it (dmabuf-protocol baseline).
        let t = fake_table(vec![], vec![DRM_FORMAT_MOD_LINEAR]);
        assert_eq!(
            t.choose().unwrap(),
            ChosenModifier {
                drm_format: DRM_FORMAT_ABGR8888,
                drm_modifier: DRM_FORMAT_MOD_LINEAR,
            }
        );
    }

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
}
