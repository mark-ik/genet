/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared serval-on-winit host plumbing.
//!
//! The meerkat chrome shell and the orrery host are both "a serval surface
//! presented on a winit window via netrender". The present mechanics — booting
//! wgpu + a netrender [`Renderer`], configuring the surface, rasterizing a
//! [`Scene`] into an offscreen texture, acquiring + compositing onto the
//! backbuffer — and the winit→serval key / modifier mapping are identical
//! between them, so they live here. Each host keeps only its own scene
//! composition and input routing.
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

use std::sync::Arc;

use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};
use winit::event::MouseScrollDelta;
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::Window;
use xilem_serval::{Key, KeyEvent, Modifiers, NamedKey};

/// The shared present core: one wgpu device + netrender [`Renderer`], booted once
/// and shared across **every** window. Per-window [`WindowSurface`]s are created
/// from it via [`create_surface`](Self::create_surface), so N windows present
/// through one device — a node texture rasterized once can be sampled into any
/// window's swapchain without re-rendering. (Multi-window: one device, N surfaces.)
pub struct RenderCore {
    renderer: Renderer,
}

impl RenderCore {
    /// Boot wgpu + a netrender [`Renderer`] (native blocking). The device is shared;
    /// create per-window surfaces with [`create_surface`](Self::create_surface). On
    /// wasm the WebGPU device request is async, so use [`boot_async`](Self::boot_async).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn boot(options: NetrenderOptions) -> Result<Self, String> {
        // `options.backends` lets a host force a backend (e.g. D3D12 for same-API
        // system-WebView import); `None` honors `WGPU_BACKEND`, else all available.
        let handles = match options.backends {
            Some(b) => netrender::boot_with(b),
            None => netrender::boot(),
        }
        .map_err(|e| format!("netrender wgpu boot failed: {e}"))?;
        Self::from_handles(handles, options)
    }

    /// Async boot: awaits netrender's `boot_async`. The only boot path on wasm
    /// (WebGPU device acquisition is asynchronous); works on every target.
    pub async fn boot_async(options: NetrenderOptions) -> Result<Self, String> {
        let handles = match options.backends {
            Some(b) => netrender::boot_async_with(b).await,
            None => netrender::boot_async().await,
        }
        .map_err(|e| format!("netrender wgpu boot failed: {e}"))?;
        Self::from_handles(handles, options)
    }

    fn from_handles(
        handles: netrender::WgpuHandles,
        options: NetrenderOptions,
    ) -> Result<Self, String> {
        let renderer = netrender::create_netrender_instance(handles, options)
            .map_err(|e| format!("netrender init failed: {e:?}"))?;
        Ok(Self { renderer })
    }

    /// Create + configure a swapchain surface for `window` at `(width, height)`,
    /// sharing this core's device. Prefers an sRGB format, else the first
    /// advertised. The surface is created from the core's retained wgpu instance,
    /// so every window draws through the one device.
    pub fn create_surface(
        &self,
        window: Arc<Window>,
        width: u32,
        height: u32,
    ) -> Result<WindowSurface, String> {
        let core = &self.renderer.wgpu_device.core;
        let surface = core
            .instance
            .create_surface(window)
            .map_err(|e| format!("create_surface failed: {e}"))?;
        let caps = surface.get_capabilities(&core.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&core.device, &surface_config);
        Ok(WindowSurface { surface, surface_config })
    }

    /// The netrender renderer — call `compose_external_texture` (and friends) on
    /// it to composite rasterized layers onto a window's backbuffer.
    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// The shared wgpu device backing the renderer.
    pub fn device(&self) -> &wgpu::Device {
        &self.renderer.wgpu_device.core.device
    }

    /// The shared wgpu queue (e.g. for external-texture import).
    pub fn queue(&self) -> &wgpu::Queue {
        &self.renderer.wgpu_device.core.queue
    }

    /// Rasterize `scene` into a fresh `(w, h)` `Rgba8Unorm` texture, cleared to
    /// `clear`. Returns the texture with its view; keep the texture alive until
    /// the composite pass has sampled the view. Device-only, so any window's frame
    /// can composite the result.
    pub fn rasterize(
        &self,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let device = self.device();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("serval-winit-host scene"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor {
            label: Some("serval-winit-host scene view"),
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        self.renderer.render_vello(scene, &view, clear);
        (tex, view)
    }
}

/// One window's swapchain surface + its configuration, created from a shared
/// [`RenderCore`]. Per-window; the device behind it is the core's, so the methods
/// that touch the device ([`resize`](Self::resize) / [`acquire`](Self::acquire))
/// take the `&RenderCore` back. (One device, N surfaces.)
pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
}

impl WindowSurface {
    /// The surface's texture format (pass to `compose_external_texture`).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// Reconfigure the surface for a new size (clamped to ≥ 1), via the shared
    /// core's device.
    pub fn resize(&mut self, core: &RenderCore, width: u32, height: u32) {
        self.surface_config.width = width.max(1);
        self.surface_config.height = height.max(1);
        self.surface.configure(core.device(), &self.surface_config);
    }

    /// Acquire this window's backbuffer for the frame. Returns `None` (and
    /// reconfigures via the shared core's device) when the surface is outdated /
    /// lost or otherwise unavailable, so the caller simply skips the frame. Stays
    /// non-blocking so a slow window never stalls another on the shared loop.
    pub fn acquire(&self, core: &RenderCore) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(core.device(), &self.surface_config);
                None
            },
            other => {
                eprintln!("[serval-winit-host] surface acquire skipped: {other:?}");
                None
            },
        }
    }
}

/// A [`RenderCore`] + its one [`WindowSurface`]: the single-window present stack,
/// kept as a convenience for hosts that only ever have one window (the standalone
/// orrery host). Multi-window meerkat holds a shared `RenderCore` + a `WindowSurface`
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

    /// Acquire the surface backbuffer for this frame (`None` to skip on outdated).
    pub fn acquire(&self) -> Option<wgpu::SurfaceTexture> {
        self.surface.acquire(&self.core)
    }
}

/// Map a winit logical key + modifiers to a serval [`KeyEvent`]; `None` for dead
/// / unidentified keys with no routable mapping.
pub fn key_event_from_winit(key: &WinitKey, mods: Modifiers) -> Option<KeyEvent> {
    let mapped = match key {
        WinitKey::Character(s) => Key::Character(s.to_string()),
        WinitKey::Named(named) => Key::Named(match named {
            WinitNamedKey::Backspace => NamedKey::Backspace,
            WinitNamedKey::Enter => NamedKey::Enter,
            WinitNamedKey::Tab => NamedKey::Tab,
            WinitNamedKey::Escape => NamedKey::Escape,
            WinitNamedKey::Space => NamedKey::Space,
            WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
            WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
            WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
            WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
            WinitNamedKey::Delete => NamedKey::Delete,
            WinitNamedKey::Home => NamedKey::Home,
            WinitNamedKey::End => NamedKey::End,
            _ => NamedKey::Other,
        }),
        WinitKey::Dead(_) | WinitKey::Unidentified(_) => return None,
    };
    Some(KeyEvent::with_mods(mapped, mods))
}

/// Map winit's modifier state to serval's [`Modifiers`].
pub fn modifiers_from_winit(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
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
/// action (scope doc rule 5): pelt and meerkat map the wheel through this one
/// helper, not two hand-rolled copies.
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
        assert!((down - WHEEL_LINE_PX).abs() < 0.01, "one line down = +{WHEEL_LINE_PX}px, got {down}");
        let (_, up) = wheel_delta_from_winit(MouseScrollDelta::LineDelta(0.0, 1.0));
        assert!((up + WHEEL_LINE_PX).abs() < 0.01, "one line up = -{WHEEL_LINE_PX}px, got {up}");
    }

    /// Pixel deltas (trackpads) pass through unscaled, sign-flipped.
    #[test]
    fn wheel_pixel_delta_passes_through() {
        let got =
            wheel_delta_from_winit(MouseScrollDelta::PixelDelta(PhysicalPosition::new(3.0, -10.0)));
        assert_eq!(got, (-3.0, 10.0));
    }
}
