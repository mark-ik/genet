/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Direction-neutral wgpu/native interop primitives for genet's C4
//! compositor adapter.
//!
//! ## Provenance
//!
//! These primitives are the slint-example → `wgpu-graft` →
//! `scrying` → genet lineage; the direction-neutral pieces
//! survived four iterations and an export/import direction
//! reversal. See
//! [`docs/2026-05-09_interop_lineage.md`](../../../docs/2026-05-09_interop_lineage.md)
//! for the full story and for why the WNTI / scrying
//! `InteropSynchronizer` trait shape doesn't fit the export
//! direction.
//!
//! ## What's here
//!
//! - [`InteropBackend`] — discriminator (Vulkan / Metal / Dx12 /
//!   Unknown), detected from a `wgpu::Device` via [`detect_backend`].
//! - [`HostWgpuContext`] — bundles `device + queue + backend`. Held
//!   by `ServoCompositor`, passed to backends.
//! - [`SyncMechanism`] — kind of producer→consumer fence machinery
//!   in play.
//! - [`InteropError`] — error enum.
//! - [`Dx12FenceSynchronizer`] — Windows-only. Wraps a
//!   `D3D12_FENCE_FLAG_SHARED` fence with `advance` / `current_value`
//!   inherent methods. Mac and Linux synchronizer wrappers are
//!   pending — see the lineage brief.

#![deny(unsafe_op_in_unsafe_fn)]

use std::fmt;

// =============================================================================
// Backend discriminator
// =============================================================================

/// The wgpu graphics backend in use on the host device.
///
/// Detected automatically by [`HostWgpuContext::new`] via `as_hal`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InteropBackend {
    /// Vulkan (Linux, Android, Windows with `wgpu::Backends::VULKAN`).
    Vulkan,
    /// Metal (macOS, iOS).
    Metal,
    /// Direct3D 12 (Windows default).
    Dx12,
    /// Backend could not be detected.
    Unknown,
}

/// Detect which wgpu backend a `wgpu::Device` is running on.
///
/// Structurally the same `as_hal::<api::*>()` probe used in the
/// graft / scrying iterations of this primitive (see
/// [`docs/2026-05-09_interop_lineage.md`](../../../docs/2026-05-09_interop_lineage.md)).
pub fn detect_backend(device: &wgpu::Device) -> InteropBackend {
    unsafe {
        // wgpu::wgc::api::Vulkan is only compiled in when the hal
        // `vulkan` cfg is set — Linux, Android, Windows (not macOS).
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "windows"))]
        if device.as_hal::<wgpu::wgc::api::Vulkan>().is_some() {
            return InteropBackend::Vulkan;
        }

        #[cfg(target_vendor = "apple")]
        if device.as_hal::<wgpu::wgc::api::Metal>().is_some() {
            return InteropBackend::Metal;
        }

        #[cfg(target_os = "windows")]
        if device.as_hal::<wgpu::wgc::api::Dx12>().is_some() {
            return InteropBackend::Dx12;
        }
    }

    InteropBackend::Unknown
}

// =============================================================================
// Host context bundle
// =============================================================================

/// Wraps a `wgpu::Device` + `wgpu::Queue` together with the detected
/// backend. Held by `ServoCompositor`; passed to
/// `OsCompositorBackend::declare` so backends can encode their own GPU
/// work.
#[derive(Clone, Debug)]
pub struct HostWgpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub backend: InteropBackend,
}

impl HostWgpuContext {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            backend: detect_backend(&device),
            device,
            queue,
        }
    }
}

// =============================================================================
// Synchronization
// =============================================================================

/// How the producer signals that a frame is ready and how the
/// consumer signals that it has finished reading.
///
/// In genet's C4 export direction, the **producer** is netrender's
/// wgpu queue (signalling at submit time) and the **consumer** is the
/// OS compositor (waiting before sampling the surface).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SyncMechanism {
    /// No synchronization needed (single-threaded path, or the
    /// producer and consumer share a queue).
    None,
    /// An explicit Vulkan / Metal external semaphore is signalled by
    /// the producer.
    ExplicitExternalSemaphore,
    /// An explicit GPU fence (D3D12) is signalled by the producer.
    ExplicitFence,
}

// =============================================================================
// Error
// =============================================================================

/// Errors raised by interop construction or per-frame synchronization.
#[derive(Debug)]
#[non_exhaustive]
pub enum InteropError {
    /// The wgpu backend on the host device does not match what this
    /// code path requires.
    BackendMismatch {
        expected: &'static str,
        actual: &'static str,
    },
    /// A D3D12 / DXGI API call failed.
    Dx12(String),
    /// A Vulkan API call failed.
    Vulkan(String),
    /// A Metal API call failed.
    Metal(String),
    /// The synchronizer received a [`SyncMechanism`] it does not
    /// handle.
    UnsupportedSynchronization(SyncMechanism),
}

impl fmt::Display for InteropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendMismatch { expected, actual } => {
                write!(f, "backend mismatch: expected {expected}, found {actual}")
            },
            Self::Dx12(msg) => write!(f, "D3D12 interop failed: {msg}"),
            Self::Vulkan(msg) => write!(f, "Vulkan interop failed: {msg}"),
            Self::Metal(msg) => write!(f, "Metal interop failed: {msg}"),
            Self::UnsupportedSynchronization(m) => {
                write!(f, "unsupported synchronization mechanism: {m:?}")
            },
        }
    }
}

impl std::error::Error for InteropError {}

// =============================================================================
// Per-platform fence machinery
// =============================================================================

#[cfg(target_os = "windows")]
mod windows_dx12;

#[cfg(target_os = "windows")]
pub use windows_dx12::Dx12FenceSynchronizer;

#[cfg(target_os = "linux")]
mod vulkan_timeline;

#[cfg(target_os = "linux")]
pub use vulkan_timeline::VulkanTimelineSemaphoreSynchronizer;

// macOS counterpart will land alongside its respective
// `OsCompositorBackend` impl. No trait shape here — backends call
// into per-platform synchronizers via inherent methods, so the
// import-direction-coupled `InteropSynchronizer` trait the upstream
// iterations carried doesn't apply. See the lineage brief at
// `docs/2026-05-09_interop_lineage.md` for the full reasoning.
