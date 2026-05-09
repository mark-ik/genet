/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C4 stubs — concrete rendering-context types.
//!
//! Pre-cut, this module hosted `WindowRenderingContext` /
//! `OffscreenRenderingContext` / `SoftwareRenderingContext` plus the
//! legacy `RenderingContext` trait. C1 deleted the GL/surfman corpus
//! the legacy types depended on; this module reinstates the names as
//! thin stubs so that `components/servo/` and embedder examples
//! compile against the post-cut shape.
//!
//! What the stubs do:
//! - [`RenderingContext`] — thin re-export of the
//!   [`RenderingContextCore`] trait under its historical name.
//! - [`WindowRenderingContext`] / [`OffscreenRenderingContext`] /
//!   [`SoftwareRenderingContext`] — placeholder structs that implement
//!   `RenderingContextCore` with `unimplemented!()` bodies. The wgpu
//!   path is in [`crate::wgpu_rendering_context::WgpuRenderingContext`];
//!   the per-shape concrete impls land alongside C4's compositor work.
//!
//! See [`docs/2026-05-08_c3_landed_notes.md`](../../../docs/2026-05-08_c3_landed_notes.md)
//! and the C4 cut-plan section for the design.

#![deny(unsafe_code)]

use std::rc::Rc;

use dpi::PhysicalSize;
use embedder_traits::RefreshDriver;
use image::RgbaImage;
use paint_types::units::DeviceIntRect;

use crate::rendering_context_core::{RenderingContextCore, WindowHandles};

/// Re-export of the [`RenderingContextCore`] trait under its historical
/// name. Pre-cut, this was a separate trait that bundled the
/// rendering-context contract with a `GlCapability`-shaped surface; the
/// post-cut shape has the same contract on `RenderingContextCore` plus
/// optional capability traits.
pub trait RenderingContext: RenderingContextCore {}

impl<T: RenderingContextCore + ?Sized> RenderingContext for T {}

/// Window-backed rendering context. Stub — the live wgpu impl is in
/// [`crate::wgpu_rendering_context::WgpuRenderingContext`].
pub struct WindowRenderingContext;

impl WindowRenderingContext {
    pub fn new(_handles: WindowHandles, _size: PhysicalSize<u32>) -> Result<Self, &'static str> {
        Err("WindowRenderingContext is a C4 stub; use WgpuRenderingContext")
    }
}

impl RenderingContextCore for WindowRenderingContext {
    fn size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(0, 0)
    }
    fn resize(&self, _size: PhysicalSize<u32>) {}
    fn present(&self) {}
    fn read_to_image(&self, _rect: DeviceIntRect) -> Option<RgbaImage> {
        None
    }
    fn refresh_driver(&self) -> Option<Rc<dyn RefreshDriver>> {
        None
    }
}

/// Offscreen rendering context. Stub.
pub struct OffscreenRenderingContext;

impl OffscreenRenderingContext {
    pub fn new(_size: PhysicalSize<u32>) -> Result<Self, &'static str> {
        Err("OffscreenRenderingContext is a C4 stub")
    }
}

impl RenderingContextCore for OffscreenRenderingContext {
    fn size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(0, 0)
    }
    fn resize(&self, _size: PhysicalSize<u32>) {}
    fn present(&self) {}
    fn read_to_image(&self, _rect: DeviceIntRect) -> Option<RgbaImage> {
        None
    }
}

/// Software (CPU rasterization) rendering context. Stub. Pre-cut this
/// was the swrast-backed context used by integration tests; with the
/// netrender path being wgpu-only, software rendering needs a
/// readback-from-GPU implementation that lands alongside the C4
/// renderer wiring.
pub struct SoftwareRenderingContext {
    size: PhysicalSize<u32>,
}

impl SoftwareRenderingContext {
    pub fn new(size: PhysicalSize<u32>) -> Result<Self, &'static str> {
        Ok(Self { size })
    }

    /// Pre-cut this returned an `Option<&dyn GlCapability>`. The cut
    /// kept the method name for source-compat with the integration
    /// tests; the returned [`GlCapability`] is itself a stub and its
    /// `make_current()` always succeeds (no GL context is actually
    /// made current).
    pub fn gl(&self) -> Option<crate::rendering_context_core::GlCapabilityHandle<'_>> {
        Some(crate::rendering_context_core::GlCapabilityHandle::new())
    }
}

impl RenderingContextCore for SoftwareRenderingContext {
    fn size(&self) -> PhysicalSize<u32> {
        self.size
    }
    fn resize(&self, _size: PhysicalSize<u32>) {}
    fn present(&self) {}
    fn read_to_image(&self, _rect: DeviceIntRect) -> Option<RgbaImage> {
        None
    }
}
