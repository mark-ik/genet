/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Direction-neutral D3D12 fence wrapper for genet's C4 compositor
//! adapter.
//!
//! Pairs the wgpu D3D12 device with an OS-compositor consumer
//! (typically DXGI Composition) via a `D3D12_FENCE_FLAG_SHARED` fence:
//!
//! 1. Construct: creates a shared fence on the wgpu D3D12 device and
//!    exports an NT handle.
//! 2. The OS-side consumer opens its own reference to the fence (e.g.
//!    `ID3D11Device5::OpenSharedFence` or `ID3D12Device::OpenSharedHandle`)
//!    via [`Dx12FenceSynchronizer::shared_handle`].
//! 3. Per frame, the producer (netrender's wgpu queue) calls
//!    [`Dx12FenceSynchronizer::advance`] for the next signal value
//!    and signals the fence at that value when its render submit
//!    completes.
//! 4. The consumer reads `current_value` and waits on its own queue.
//!
//! Mirrors the inherent-method surface of WNTI's
//! `Dx12FenceSynchronizer`. Differs in that there is no
//! `InteropSynchronizer` trait impl — the
//! `OsCompositorBackend::present` flow drives the fence directly via
//! these inherent methods, free from import-direction wrapping.

#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::{
    Foundation::{CloseHandle, GENERIC_ALL, HANDLE},
    Graphics::Direct3D12::{
        D3D12_FENCE_FLAG_SHARED, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
    },
};

use super::{HostWgpuContext, InteropBackend, InteropError};

/// Synchronizer that uses a shared D3D12 fence to gate consumer reads
/// on producer rendering completion.
///
/// Construct one per [`HostWgpuContext`] (i.e. per wgpu D3D12 device).
/// Reuse across frames. Pass [`shared_handle`](Self::shared_handle) to
/// the consumer once at startup; call [`advance`](Self::advance)
/// before each frame to obtain the value the producer should signal.
pub struct Dx12FenceSynchronizer {
    fence: ID3D12Fence,
    queue: ID3D12CommandQueue,
    shared_handle: HANDLE,
    next_value: AtomicU64,
}

unsafe impl Send for Dx12FenceSynchronizer {}
unsafe impl Sync for Dx12FenceSynchronizer {}

impl Dx12FenceSynchronizer {
    /// Create a new shared fence on the host's wgpu D3D12 device and
    /// export an NT handle for the consumer.
    ///
    /// Returns [`InteropError::BackendMismatch`] if `host.backend` is
    /// not [`InteropBackend::Dx12`].
    pub fn new(host: &HostWgpuContext) -> Result<Self, InteropError> {
        if host.backend != InteropBackend::Dx12 {
            return Err(InteropError::BackendMismatch {
                expected: "Dx12",
                actual: "non-Dx12",
            });
        }

        let (fence, queue, shared_handle) = unsafe {
            let hal_device = host.device.as_hal::<wgpu::wgc::api::Dx12>().ok_or(
                InteropError::BackendMismatch {
                    expected: "Dx12",
                    actual: "non-Dx12",
                },
            )?;
            let d3d_device: ID3D12Device = hal_device.raw_device().clone();
            drop(hal_device);

            let hal_queue = host.queue.as_hal::<wgpu::wgc::api::Dx12>().ok_or(
                InteropError::BackendMismatch {
                    expected: "Dx12",
                    actual: "non-Dx12",
                },
            )?;
            let queue: ID3D12CommandQueue = hal_queue.as_raw().clone();
            drop(hal_queue);

            let fence: ID3D12Fence = d3d_device
                .CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_SHARED)
                .map_err(|err| InteropError::Dx12(format!("CreateFence: {err}")))?;

            let shared_handle = d3d_device
                .CreateSharedHandle(&fence, None, GENERIC_ALL.0, None)
                .map_err(|err| InteropError::Dx12(format!("CreateSharedHandle: {err}")))?;

            (fence, queue, shared_handle)
        };

        Ok(Self {
            fence,
            queue,
            shared_handle,
            next_value: AtomicU64::new(0),
        })
    }

    /// The shared NT handle for the consumer's `OpenSharedFence` /
    /// `OpenSharedHandle` call. The synchronizer closes the handle on
    /// drop; the consumer should `DuplicateHandle` if it needs to
    /// outlive the synchronizer.
    pub fn shared_handle(&self) -> HANDLE {
        self.shared_handle
    }

    /// Increment the fence counter and return the new value. The
    /// producer (netrender's wgpu D3D12 queue) signals at this value
    /// when the render submit completes.
    pub fn advance(&self) -> u64 {
        self.next_value.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The current fence value (highest value returned by
    /// [`advance`](Self::advance), or `0` if `advance` has not been
    /// called).
    pub fn current_value(&self) -> u64 {
        self.next_value.load(Ordering::SeqCst)
    }

    /// Queue a `Wait(value)` on the wgpu D3D12 queue itself, gating
    /// any subsequent producer-side submit on the consumer reaching
    /// `value`. Useful when the consumer signals a separate "I'm done
    /// reading" fence whose latest value the producer should wait on
    /// before re-rendering into the same surface.
    pub fn queue_wait(&self, value: u64) -> Result<(), InteropError> {
        if value == 0 {
            return Ok(());
        }
        unsafe {
            self.queue
                .Wait(&self.fence, value)
                .map_err(|err| InteropError::Dx12(format!("Wait: {err}")))?;
        }
        Ok(())
    }
}

impl Drop for Dx12FenceSynchronizer {
    fn drop(&mut self) {
        if !self.shared_handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.shared_handle);
            }
        }
    }
}
