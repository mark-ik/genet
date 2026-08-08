/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared genet-on-winit host plumbing.
//!
//! What is specific to winit lives here: the single-window [`SurfaceHost`]
//! convenience, wheel translation, and the AccessKit bridge. The present
//! mechanics under them — booting wgpu + a netrender [`Renderer`], configuring
//! a surface, rasterizing a [`Scene`], acquiring a backbuffer — are not about
//! windowing at all and now live in `genet-render-host`, which a browser
//! canvas host reaches without pulling winit or accesskit into its build.
//! [`RenderCore`] and [`WindowSurface`] are re-exported here so existing
//! callers are unaffected.
//!
//! Each host keeps its own scene composition and input routing. Cambium
//! keyboard translation lives in `cambium-winit`; this engine host does not
//! depend on the GUI layer.
//!
//! Per-frame shape a host follows:
//!
//! ```text
//! let (_tex, view) = host.rasterize(&scene, w, h, clear);   // one per layer
//! let Some(frame)  = host.acquire() else { return };         // skip if outdated
//! let target = frame.texture.create_view(&Default::default());
//! host.renderer().compose_external_texture(&view, &target, host.format(), w, h, placement);
//! frame.present();
//! ```

mod a11y;
pub use a11y::{A11yActionRequest, AccessKitBridge, BridgeStatus};

use std::sync::Arc;

use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};
use winit::event::MouseScrollDelta;
use winit::window::Window;

/// The target-neutral present core. Re-exported so hosts that already speak
/// `genet_winit_host::RenderCore` keep working; new target-neutral code should
/// depend on `genet-render-host` directly.
pub use genet_render_host::{RenderCore, WindowSurface};

/// A [`RenderCore`] + its one [`WindowSurface`]: the single-window present stack,
/// kept as a convenience for hosts that only ever have one window (the standalone
/// orrery host). A multi-window host holds a shared `RenderCore` + a `WindowSurface`
/// per window directly. The per-frame shape is unchanged — `rasterize` each scene,
/// `acquire` the backbuffer, composite, present.
pub struct SurfaceHost {
    core: RenderCore,
    surface: WindowSurface,
}

impl SurfaceHost {
    /// Boot the core + create this window's surface (native blocking).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn boot(
        window: Arc<Window>,
        width: u32,
        height: u32,
        options: NetrenderOptions,
    ) -> Result<Self, String> {
        let core = RenderCore::boot(options)?;
        let surface = core.create_surface(window, width, height)?;
        Ok(Self { core, surface })
    }

    /// Async boot (the only path on wasm; works everywhere).
    pub async fn boot_async(
        window: Arc<Window>,
        width: u32,
        height: u32,
        options: NetrenderOptions,
    ) -> Result<Self, String> {
        let core = RenderCore::boot_async(options).await?;
        let surface = core.create_surface(window, width, height)?;
        Ok(Self { core, surface })
    }

    /// The shared render core (device + renderer).
    pub fn core(&self) -> &RenderCore {
        &self.core
    }

    /// The netrender renderer — call `compose_external_texture` (and friends) on it.
    pub fn renderer(&self) -> &Renderer {
        self.core.renderer()
    }

    /// The surface's texture format (pass to `compose_external_texture`).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.surface.format()
    }

    /// The wgpu device backing the renderer.
    pub fn device(&self) -> &wgpu::Device {
        self.core.device()
    }

    /// The wgpu queue backing the renderer (e.g. for external-texture import).
    pub fn queue(&self) -> &wgpu::Queue {
        self.core.queue()
    }

    /// Reconfigure the surface for a new size (clamped to ≥ 1).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize(&self.core, width, height);
    }

    /// Rasterize `scene` into a fresh `(w, h)` texture cleared to `clear`.
    pub fn rasterize(
        &self,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        self.core.rasterize(scene, w, h, clear)
    }

    /// Rasterize a logical-coordinate scene into a physical `(w, h)` texture at
    /// `scale` device pixels per logical pixel. The same scale must be used for
    /// layout and hit testing by the caller; this wrapper keeps the window host
    /// on the crisp renderer path instead of upscaling a low-resolution texture.
    pub fn rasterize_scaled(
        &self,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
        scale: f32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        self.core.rasterize_scaled(scene, w, h, clear, scale)
    }

    /// Acquire the surface backbuffer for this frame (`None` to skip on outdated).
    pub fn acquire(&self) -> Option<wgpu::SurfaceTexture> {
        self.surface.acquire(&self.core)
    }
}

/// Device px per wheel "line" step, for `MouseScrollDelta::LineDelta` events
/// (mouse wheels report lines, trackpads report pixels). One notch ≈ a few lines.
pub const WHEEL_LINE_PX: f32 = 48.0;

/// Map a winit wheel event to a device-px delta to **add** to a document's
/// viewport scroll (`viewport.scroll += delta`). A line step scales by
/// [`WHEEL_LINE_PX`]; a pixel step (trackpad) passes through. The sign is flipped
/// from winit's "positive = content moves up / away", so rolling the wheel down
/// advances the document toward its end (a larger offset). The shared wheel default
/// action (scope doc rule 5): every winit host maps the wheel through this one
/// helper, not several hand-rolled copies.
pub fn wheel_delta_from_winit(delta: MouseScrollDelta) -> (f32, f32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (-x * WHEEL_LINE_PX, -y * WHEEL_LINE_PX),
        MouseScrollDelta::PixelDelta(p) => (-(p.x as f32), -(p.y as f32)),
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    /// A line step scales to `WHEEL_LINE_PX` with the sign flipped: rolling the
    /// wheel down (winit y < 0) advances the document (positive dy), up reverses it.
    #[test]
    fn wheel_line_delta_maps_to_document_scroll() {
        let (dx, down) = wheel_delta_from_winit(MouseScrollDelta::LineDelta(0.0, -1.0));
        assert_eq!(dx, 0.0);
        assert!(
            (down - WHEEL_LINE_PX).abs() < 0.01,
            "one line down = +{WHEEL_LINE_PX}px, got {down}"
        );
        let (_, up) = wheel_delta_from_winit(MouseScrollDelta::LineDelta(0.0, 1.0));
        assert!(
            (up + WHEEL_LINE_PX).abs() < 0.01,
            "one line up = -{WHEEL_LINE_PX}px, got {up}"
        );
    }

    /// Pixel deltas (trackpads) pass through unscaled, sign-flipped.
    #[test]
    fn wheel_pixel_delta_passes_through() {
        let got = wheel_delta_from_winit(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            3.0, -10.0,
        )));
        assert_eq!(got, (-3.0, 10.0));
    }
}
