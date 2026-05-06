/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Backend-neutral rendering-context contract.
//!
//! Post-cut (2026-05-05): the GL/surfman corpus is gone. The legacy
//! `RenderingContext` trait and `GlCapability` capability are deleted;
//! `WgpuCapability` is the only capability and `WgpuRenderingContext`
//! is the only concrete impl. Future window/offscreen rendering
//! contexts implement [`RenderingContextCore`] + [`WgpuCapability`].

#![deny(unsafe_code)]

use std::rc::Rc;

use dpi::PhysicalSize;
use embedder_traits::RefreshDriver;
use euclid::Size2D;
use image::RgbaImage;
use paint_types::units::{DeviceIntRect, DevicePixel};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

/// Bundled raw window + display handles for creating a platform surface.
///
/// Two handles are always used together for wgpu surface creation, so
/// bundling them removes the double-unwrap at every call site.
#[derive(Debug, Clone, Copy)]
pub struct WindowHandles {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}

/// Core rendering-context contract. Backend-neutral.
///
/// Every concrete rendering-context type implements this trait and
/// optionally exposes a [`WgpuCapability`] (via [`wgpu`]).
///
/// [`wgpu`]: RenderingContextCore::wgpu
pub trait RenderingContextCore {
    // --- Geometry + presentation ---

    fn size(&self) -> PhysicalSize<u32>;

    fn size2d(&self) -> Size2D<u32, DevicePixel> {
        let s = self.size();
        Size2D::new(s.width, s.height)
    }

    fn resize(&self, size: PhysicalSize<u32>);

    fn present(&self);

    /// Read a viewport-space rectangle of the current rendered frame
    /// into an in-memory image.
    ///
    /// `rect` uses a top-left origin in device pixels. The bytes
    /// reflect the compositor output as stored in the swapchain
    /// texture; depending on surface format, golden-image comparisons
    /// may need tolerance for sRGB encoding and premultiplied-alpha
    /// differences.
    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage>;

    // --- Window integration (optional; offscreen contexts return None) ---

    /// Raw window + display handles, bundled.
    fn window_handles(&self) -> Option<WindowHandles> {
        None
    }

    /// Host-provided refresh driver; `None` means the default
    /// timer-based driver is used.
    fn refresh_driver(&self) -> Option<Rc<dyn RefreshDriver>> {
        None
    }

    // --- Capability objects ---

    /// wgpu capability — required for any context driving the wgpu
    /// compositor path. `None` for legacy / stub contexts.
    #[cfg(feature = "wgpu_backend")]
    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        None
    }
}

/// Capability surface for wgpu-backed rendering contexts. Accessed via
/// [`RenderingContextCore::wgpu`]. Holding an `&dyn WgpuCapability`
/// proves at the type level that the context can drive a wgpu compositor.
#[cfg(feature = "wgpu_backend")]
pub trait WgpuCapability {
    /// Clone of the context's wgpu device. The device handle is internally
    /// `Arc`-shared, so cloning is cheap and returned handles operate on
    /// the same GPU context.
    fn device(&self) -> wgpu::Device;

    /// Clone of the context's wgpu queue. Paired with `device()`.
    fn queue(&self) -> wgpu::Queue;

    /// Acquire the next swapchain texture for this frame. Returns the
    /// texture view the compositor should draw into. `None` means the
    /// swapchain couldn't acquire a target (lost / suboptimal surface).
    fn acquire_frame_target(&self) -> Option<wgpu::TextureView>;

    /// Optional factory hook for embedders that hold a raw `wgpu_hal`
    /// device and want the compositor to wrap it via
    /// `Adapter::create_device_from_hal` rather than creating its own
    /// device stack. Takes precedence over `device()`/`queue()` when
    /// both are provided.
    fn hal_device_factory(
        &self,
    ) -> Option<Box<dyn FnOnce() -> (wgpu::Device, wgpu::Queue) + Send>> {
        None
    }
}
