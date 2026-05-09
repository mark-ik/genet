/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Direction-neutral wgpu/native interop primitives for serval's C4
//! compositor adapter.
//!
//! ## Why this lives here (and not in `wgpu-native-texture-interop`)
//!
//! `wgpu-native-texture-interop` (WNTI) ships a sibling set of types
//! oriented toward the **import** direction (system-webview producer
//! → wgpu consumer). C4's [`crate::compositor`] needs the **export**
//! direction (netrender producer → OS-compositor consumer), which
//! requires a different sync-trait shape:
//!
//! - WNTI's [`InteropSynchronizer::producer_complete`] takes a
//!   `&NativeFrame` (an enum of producer-side handles) and
//!   `consumer_ready` takes a `&ImportedTexture`. Neither fits the
//!   export path — there is no `NativeFrame` from a producer; the
//!   producer is netrender's own wgpu queue.
//! - The remaining direction-neutral pieces (`InteropBackend`,
//!   `HostWgpuContext`, `SyncMechanism`, the platform fence
//!   wrappers) are small enough to host inline rather than depend on
//!   WNTI for them.
//!
//! Per project policy: serval doesn't shape WNTI to fit its needs;
//! the surface here mirrors WNTI's where it makes sense and diverges
//! where the export direction does.
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
//!   inherent methods. Direction-neutral; the trait dance is handled
//!   externally so `OsCompositorBackend` impls can call into the
//!   fence without an import-shaped wrapper.

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
/// Mirrors `wgpu-native-texture-interop`'s detect_backend; structurally
/// identical because both use the same `as_hal::<api::*>()` probe.
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
/// In serval's C4 export direction, the **producer** is netrender's
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
            }
            Self::Dx12(msg) => write!(f, "D3D12 interop failed: {msg}"),
            Self::Vulkan(msg) => write!(f, "Vulkan interop failed: {msg}"),
            Self::Metal(msg) => write!(f, "Metal interop failed: {msg}"),
            Self::UnsupportedSynchronization(m) => {
                write!(f, "unsupported synchronization mechanism: {m:?}")
            }
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

// Vulkan / Metal counterparts will land alongside their respective
// `OsCompositorBackend` impls. Trait shape is intentionally not
// declared here — `OsCompositorBackend` itself owns the per-frame
// fence dance, and per-platform synchronizers expose inherent methods
// that the backend impl calls into directly. This avoids the
// import-coupled-trait mistake that WNTI's `InteropSynchronizer`
// shape forces consumers into.
