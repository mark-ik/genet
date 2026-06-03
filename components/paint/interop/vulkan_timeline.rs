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
use std::sync::atomic::AtomicU64;

use ash::vk;

use super::{HostWgpuContext, InteropBackend, InteropError};

/// Vulkan timeline-semaphore synchronizer. Construct one per
/// [`HostWgpuContext`] (i.e. per wgpu Vulkan device). Reuse across
/// frames. Producers wire [`semaphore`](Self::semaphore) into their own
/// `vkQueueSubmit` calls; the wrapper does not issue empty-buffer signal
/// or wait submits.
#[allow(dead_code)]
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
        // Real impl lands in Task 2.2.
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
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                flags: wgpu::InstanceFlags::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                backend_options: wgpu::BackendOptions::default(),
                display: None,
            });
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
