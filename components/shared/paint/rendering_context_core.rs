/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase A: wgpu-first split of the `RenderingContext` trait.
//!
//! See `docs/2026-04-18_servo_wgpuification_plan.md` §1 and the
//! companion audit + trait-design docs. This module defines the
//! capability-object-shaped successor to the legacy
//! [`crate::rendering_context::RenderingContext`] trait.
//!
//! During Phase A, both traits coexist: the legacy trait is still
//! implemented by every concrete context type, and consumers migrate
//! batch by batch from the legacy trait to [`RenderingContextCore`]
//! + the capability objects. At the end of Phase A the legacy trait
//! is deleted.
//!
//! Design summary:
//!
//! - [`RenderingContextCore`] holds only the methods every rendering
//!   context must implement (geometry, presentation, readback, window
//!   handles). No GL or wgpu methods are required.
//! - [`WgpuCapability`] and [`GlCapability`] are optional capability
//!   traits accessed via `ctx.wgpu()` / `ctx.gl()`. Holding an
//!   `&dyn WgpuCapability` proves at the type level that the context
//!   can drive a wgpu compositor; the legacy trait required every
//!   impl to implement `gleam_gl_api` / `glow_gl_api`, forcing
//!   `WgpuRenderingContext` to carry `unreachable!()` panic stubs.
//!   The capability split eliminates that panic surface by
//!   construction.

#![deny(unsafe_code)]

use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use embedder_traits::RefreshDriver;
use euclid::Size2D;
use euclid::default::Size2D as UntypedSize2D;
use image::RgbaImage;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use surfman::{Connection, Error, Surface, SurfaceTexture};
use webrender_api::units::{DeviceIntRect, DevicePixel};

/// Bundled raw window + display handles for creating a platform surface.
///
/// The legacy trait exposed `raw_window_handle()` and `raw_display_handle()`
/// as two separate optional methods. They are always used together for wgpu
/// surface creation, so bundling them into one `Option<WindowHandles>`
/// removes the double-unwrap at every call site.
#[derive(Debug, Clone, Copy)]
pub struct WindowHandles {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}

/// Core rendering-context contract. Backend-neutral.
///
/// Every concrete rendering-context type implements this trait and
/// optionally exposes a [`WgpuCapability`] (via [`wgpu`]) and/or a
/// [`GlCapability`] (via [`gl`]).
///
/// [`wgpu`]: RenderingContextCore::wgpu
/// [`gl`]: RenderingContextCore::gl
pub trait RenderingContextCore {
    // --- Geometry + presentation (all backends) ---

    fn size(&self) -> PhysicalSize<u32>;

    fn size2d(&self) -> Size2D<u32, DevicePixel> {
        let s = self.size();
        Size2D::new(s.width, s.height)
    }

    fn resize(&self, size: PhysicalSize<u32>);

    fn present(&self);

    /// Read the current back-buffer into an in-memory image. Backend-neutral:
    /// GL impls use `glReadPixels`; wgpu impls use a staging buffer +
    /// map-read (currently stubbed in `WgpuRenderingContext`; tracked as a
    /// separate follow-on).
    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage>;

    // --- Window integration (optional; offscreen contexts return None) ---

    /// Raw window + display handles, bundled.
    fn window_handles(&self) -> Option<WindowHandles> {
        None
    }

    /// Host-provided refresh driver; `None` means the default timer-based
    /// driver is used. Not backend-specific.
    fn refresh_driver(&self) -> Option<Rc<dyn RefreshDriver>> {
        None
    }

    // --- Capability objects ---

    /// wgpu capability — required for any context driving the wgpu
    /// compositor path. `None` for pure-GL / software legacy contexts.
    #[cfg(feature = "wgpu_backend")]
    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        None
    }

    /// GL capability — required for WebGL, WebXR (current), and
    /// `egui_glow`-style embedder chrome. `None` for wgpu-first
    /// contexts that never expose GL.
    fn gl(&self) -> Option<&dyn GlCapability> {
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

    /// Acquire the next swapchain texture for this frame. Wgpu equivalent
    /// of GL's `prepare_for_rendering` — returns the texture view the
    /// compositor should draw into. `None` means the swapchain couldn't
    /// acquire a target (lost / suboptimal surface).
    fn acquire_frame_target(&self) -> Option<wgpu::TextureView>;

    /// Optional factory hook for embedders that hold a raw `wgpu_hal`
    /// device and want WebRender to wrap it via `Adapter::create_device_from_hal`
    /// rather than creating its own device stack. Called at most once
    /// during `create_webrender_instance_with_backend`; takes `&self`
    /// but uses interior mutability to consume the factory.
    ///
    /// Takes precedence over `device()`/`queue()` when both are provided.
    fn hal_device_factory(
        &self,
    ) -> Option<Box<dyn FnOnce() -> (wgpu::Device, wgpu::Queue) + Send>> {
        None
    }
}

/// Capability surface for GL-backed rendering contexts. Accessed via
/// [`RenderingContextCore::gl`]. Holding an `&dyn GlCapability` proves
/// at the type level that the context has a current GL context available.
pub trait GlCapability {
    /// Make the GL context current on the calling thread.
    fn make_current(&self) -> Result<(), Error>;

    /// The `gleam`-flavored GL API handle.
    fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl>;

    /// The `glow`-flavored GL API handle.
    fn glow_gl_api(&self) -> Arc<glow::Context>;

    /// Prepare the GL framebuffer for rendering (binds the Surfman-owned
    /// framebuffer). No wgpu equivalent; wgpu's frame target is acquired
    /// per-frame via [`WgpuCapability::acquire_frame_target`] instead.
    fn prepare_for_rendering(&self);

    /// Wrap a Surfman surface as a GL surface texture. Returns the surface
    /// texture, the underlying GL texture name, and the size. Used by the
    /// GL external-image import path (WebGL → compositor). `None` if the
    /// backend doesn't support surface-texture wrapping.
    fn create_texture(
        &self,
        surface: Surface,
    ) -> Option<(SurfaceTexture, u32, UntypedSize2D<i32>)>;

    /// Release a previously created surface texture and return the
    /// underlying Surfman surface for recycling.
    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface>;

    /// The Surfman connection backing this GL context. Non-optional on
    /// `GlCapability`: if you have a GL capability, you have a connection.
    /// (The legacy trait had `connection() -> Option<Connection>` because
    /// wgpu-only impls had to satisfy the method.)
    fn connection(&self) -> Connection;
}
