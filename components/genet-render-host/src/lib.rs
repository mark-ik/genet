/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The genet present core, independent of what it presents onto.
//!
//! Booting wgpu + a netrender [`Renderer`], configuring a surface, rasterizing
//! a [`Scene`] into an offscreen texture, and acquiring a backbuffer are the
//! same mechanics whether the target is a desktop window or a browser canvas.
//! They live here so one implementation serves both; each host keeps its own
//! scene composition and input routing.
//!
//! [`RenderCore::create_surface`] takes anything wgpu accepts as a surface
//! target, so a winit `Arc<Window>` and an `HtmlCanvasElement` both work
//! without this crate depending on either. `genet-winit-host` adds the winit
//! event loop, wheel translation, and the AccessKit bridge on top.
//!
//! Per-frame shape a host follows:
//!
//! ```text
//! let (_tex, view) = core.rasterize(&scene, w, h, clear);   // one per layer
//! let Some(frame)  = surface.acquire(&core) else { return }; // skip if outdated
//! let target = frame.texture.create_view(&Default::default());
//! core.renderer().compose_external_texture(&view, &target, surface.format(), w, h, placement);
//! frame.present();
//! ```

use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};

/// The shared present core: one wgpu device + netrender [`Renderer`], booted once
/// and shared across **every** surface. Per-target [`WindowSurface`]s are created
/// from it via [`create_surface`](Self::create_surface), so N surfaces present
/// through one device — a node texture rasterized once can be sampled into any
/// surface's swapchain without re-rendering. (Multi-window: one device, N surfaces.)
pub struct RenderCore {
    renderer: Renderer,
}

impl RenderCore {
    /// Boot wgpu + a netrender [`Renderer`] (native blocking). The device is shared;
    /// create per-target surfaces with [`create_surface`](Self::create_surface). On
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

    /// Create + configure a swapchain surface for `target` at `(width, height)`,
    /// sharing this core's device. Prefers a non-sRGB format, else the first
    /// advertised. The surface is created from the core's retained wgpu instance,
    /// so every surface draws through the one device.
    ///
    /// `target` is anything wgpu turns into a surface target: a winit
    /// `Arc<Window>`, or `wgpu::SurfaceTarget::Canvas(HtmlCanvasElement)` in a
    /// browser. That is the whole of what used to tie this to a windowing library.
    pub fn create_surface(
        &self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<WindowSurface, String> {
        let core = &self.renderer.wgpu_device.core;
        let surface = core
            .instance
            .create_surface(target)
            .map_err(|e| format!("create_surface failed: {e}"))?;
        let caps = surface.get_capabilities(&core.adapter);
        // Prefer the NON-srgb surface format. The vello rasterizer writes
        // display-referred (sRGB-encoded) bytes into the Rgba8Unorm layer
        // textures and the compose pass samples them raw, so an sRGB
        // backbuffer re-encodes and washes everything out (verified against
        // the genet_web_smoke browser receipt, whose canvas surface is
        // non-srgb and renders the same scene correctly; woodshed-genet on
        // the old srgb pick showed the double-encode wash, 2026-07-05).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
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
        Ok(WindowSurface {
            surface,
            surface_config,
        })
    }

    /// The netrender renderer — call `compose_external_texture` (and friends) on
    /// it to composite rasterized layers onto a surface's backbuffer.
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
    /// the composite pass has sampled the view. Device-only, so any surface's frame
    /// can composite the result.
    pub fn rasterize(
        &self,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        self.rasterize_scaled(scene, w, h, clear, 1.0)
    }

    /// Like [`rasterize`](Self::rasterize) but with a per-SURFACE tile cache:
    /// `surface` names one retained surface (a shell partition, a canvas, one
    /// content card) with any stable host-chosen id. A host that rasterizes
    /// several surfaces through one core MUST key them — through the unkeyed
    /// entry each render diffs its scene against whichever surface rendered
    /// last, so every tile is dirty on every call (P4, shell paint plan
    /// 2026-07-03: 234/234 tiles rebuilt per settled frame).
    pub fn rasterize_for(
        &self,
        surface: u64,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        self.rasterize_scaled_for(surface, scene, w, h, clear, 1.0)
    }

    /// [`rasterize_scaled`](Self::rasterize_scaled) with a per-surface tile
    /// cache — see [`rasterize_for`](Self::rasterize_for).
    pub fn rasterize_scaled_for(
        &self,
        surface: u64,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
        scale: f32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let (tex, view) = self.scene_target(w, h);
        self.renderer
            .render_vello_scaled_for(surface, scene, &view, clear, scale);
        self.log_raster_spans(w, h);
        (tex, view)
    }

    /// Like [`rasterize`](Self::rasterize) but for a scene laid out in **logical**
    /// (DIP) coordinates: the texture is the physical `(w, h)` and `scale` is the
    /// device-pixel-ratio, so the logical scene is rasterized crisply at physical
    /// resolution (the scene's viewport must be `(w/scale, h/scale)`). The
    /// device-pixel-ratio path for HiDPI content tiles. (Auto-DPI D2.)
    pub fn rasterize_scaled(
        &self,
        scene: &Scene,
        w: u32,
        h: u32,
        clear: ColorLoad,
        scale: f32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let (tex, view) = self.scene_target(w, h);
        self.renderer
            .render_vello_scaled(scene, &view, clear, scale);
        self.log_raster_spans(w, h);
        (tex, view)
    }

    fn scene_target(&self, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let device = self.device();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("genet-render-host scene"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
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
            label: Some("genet-render-host scene view"),
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        (tex, view)
    }

    /// P3 raster attribution (shell paint plan 2026-07-03): surface the
    /// rasterizer's per-phase spans + dirty-tile count per render call, so a
    /// frame's raster cost decomposes without a debugger. Debug level; an idle
    /// app renders no frames, so there is no idle log cost.
    fn log_raster_spans(&self, w: u32, h: u32) {
        if tracing::enabled!(target: "genet_render_host::raster", tracing::Level::DEBUG) {
            if let Some(t) = self.renderer.last_frame_timings() {
                let us = |n: &str| t.span(n).map(|d| d.as_micros() as u64).unwrap_or(0);
                tracing::debug!(
                    target: "genet_render_host::raster",
                    w,
                    h,
                    total_us = t.total.as_micros() as u64,
                    tile_invalidate_us = us("tile_invalidate"),
                    dirty_tile_rebuild_us = us("dirty_tile_rebuild"),
                    master_compose_us = us("master_compose"),
                    vello_render_us = us("vello_render"),
                    dirty_tiles = self.renderer.vello_last_dirty_count().unwrap_or(0),
                    "raster spans"
                );
            }
        }
    }
}

/// One target's swapchain surface + its configuration, created from a shared
/// [`RenderCore`]. Per-target; the device behind it is the core's, so the methods
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

    /// Acquire this target's backbuffer for the frame. Returns `None` (and
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
                eprintln!("[genet-render-host] surface acquire skipped: {other:?}");
                None
            },
        }
    }
}
