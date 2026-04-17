/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebGL external image handler for the wgpu rendering backend.
//!
//! Imports WebGL canvas surfaces from surfman swap chains into wgpu textures
//! via the wgpu-native-texture-interop bridge. This replaces the GL-based
//! `WebGLExternalImages` when WebRender uses the wgpu backend.

use std::rc::Rc;

use dpi::PhysicalSize;
use log::debug;
use rustc_hash::FxHashMap;
use servo_canvas_traits::webgl::{WebGLContextId, WebGLThreads};
use surfman::chains::{SwapChainAPI, SwapChains, SwapChainsAPI};
use surfman::{Device, Surface};
use webgl::webgl_thread::WebGLContextBusyMap;
use webrender::WgpuExternalImageHandler;
use wgpu_native_texture_interop::surfman_gl::{SurfmanFrameContext, SurfmanFrameProducer};
use wgpu_native_texture_interop::{
    FrameProducer, HostWgpuContext, ImportOptions, TextureImporter, WgpuTextureImporter,
};

/// WebGL external image handler for the wgpu backend.
///
/// On `lock_wgpu()`, takes the WebGL front buffer from the swap chain, binds it
/// to a dedicated surfman GL context, and imports the GL framebuffer into a wgpu
/// texture via native interop (Vulkan external memory / Metal IOSurface / D3D12
/// shared texture).
pub struct WgpuWebGLExternalImages {
    webgl_threads: WebGLThreads,
    swap_chains: SwapChains<WebGLContextId, Device>,
    busy_webgl_context_map: WebGLContextBusyMap,

    /// Dedicated surfman GL context for binding WebGL surfaces during import.
    frame_context: Rc<SurfmanFrameContext>,
    /// Importer that converts GL framebuffers into wgpu textures.
    importer: WgpuTextureImporter,

    /// Surfaces currently locked for compositing, keyed by WebGL context ID.
    /// On unlock, these are unbound and recycled back to the swap chain.
    locked_surfaces: FxHashMap<WebGLContextId, Surface>,
}

impl WgpuWebGLExternalImages {
    pub fn new(
        webgl_threads: WebGLThreads,
        swap_chains: SwapChains<WebGLContextId, Device>,
        busy_webgl_context_map: WebGLContextBusyMap,
        wgpu_device: wgpu::Device,
        wgpu_queue: wgpu::Queue,
    ) -> Result<Self, surfman::Error> {
        let connection = surfman::Connection::new()?;
        let adapter = connection.create_adapter()?;
        let frame_context = Rc::new(SurfmanFrameContext::new(&connection, &adapter)?);
        let importer = WgpuTextureImporter::new(HostWgpuContext::new(wgpu_device, wgpu_queue));

        Ok(Self {
            webgl_threads,
            swap_chains,
            busy_webgl_context_map,
            frame_context,
            importer,
            locked_surfaces: FxHashMap::default(),
        })
    }
}

impl WgpuExternalImageHandler for WgpuWebGLExternalImages {
    fn lock_wgpu(
        &mut self,
        key: webrender_api::ExternalImageId,
        _channel_index: u8,
    ) -> Option<webrender::WgpuExternalImage> {
        let id = WebGLContextId(key.0);
        debug!("WgpuWebGLExternalImages: locking {:?}", id);

        // Mark context as busy (prevents recycling by the WebGL thread).
        {
            let mut busy = self.busy_webgl_context_map.write();
            *busy.entry(id).or_default() += 1;
        }

        // Take the front buffer from the WebGL swap chain.
        let front_buffer = self.swap_chains.get(id)?.take_surface()?;

        // Get surface size before binding.
        let size = {
            let device = self.frame_context.device.borrow();
            let info = device.surface_info(&front_buffer);
            PhysicalSize::new(info.size.width as u32, info.size.height as u32)
        };

        // Bind the WebGL surface to our dedicated GL context.
        if let Err(e) = self.frame_context.bind_surface(front_buffer) {
            log::error!("Failed to bind WebGL surface: {:?}", e);
            return None;
        }

        if let Err(e) = self.frame_context.make_current() {
            log::error!("Failed to make GL context current: {:?}", e);
            // Try to unbind and return the surface.
            if let Ok(Some(surface)) = self.frame_context.unbind_surface() {
                self.swap_chains
                    .get(id)
                    .expect("swap chain should exist")
                    .recycle_surface(surface);
            }
            return None;
        }

        // Create a frame producer and import via the bridge.
        let mut producer = SurfmanFrameProducer::new(self.frame_context.clone(), size);
        let frame = match producer.acquire_frame() {
            Ok(f) => f,
            Err(e) => {
                log::error!("Failed to acquire frame for import: {:?}", e);
                if let Ok(Some(surface)) = self.frame_context.unbind_surface() {
                    self.swap_chains
                        .get(id)
                        .expect("swap chain should exist")
                        .recycle_surface(surface);
                }
                return None;
            },
        };

        let imported = match self
            .importer
            .import_frame(&frame, &ImportOptions::default())
        {
            Ok(tex) => tex,
            Err(e) => {
                log::error!("Failed to import WebGL texture into wgpu: {:?}", e);
                // The import may or may not have re-bound the surface; try to unbind.
                if let Ok(Some(surface)) = self.frame_context.unbind_surface() {
                    self.swap_chains
                        .get(id)
                        .expect("swap chain should exist")
                        .recycle_surface(surface);
                }
                return None;
            },
        };

        // After import, the surface is re-bound. Unbind it and store for unlock.
        match self.frame_context.unbind_surface() {
            Ok(Some(surface)) => {
                self.locked_surfaces.insert(id, surface);
            },
            Ok(None) => {
                log::warn!("No surface to unbind after import for {:?}", id);
            },
            Err(e) => {
                log::error!("Failed to unbind surface after import: {:?}", e);
            },
        }

        let uv = webrender_api::units::TexelRect::new(
            0.0,
            0.0,
            imported.size.width as f32,
            imported.size.height as f32,
        );

        Some(webrender::WgpuExternalImage {
            texture: imported.texture,
            width: imported.size.width,
            height: imported.size.height,
            uv,
        })
    }

    fn unlock_wgpu(&mut self, key: webrender_api::ExternalImageId, _channel_index: u8) {
        let id = WebGLContextId(key.0);
        debug!("WgpuWebGLExternalImages: unlocking {:?}", id);

        // Decrement busy count.
        {
            let mut busy = self.busy_webgl_context_map.write();
            *busy.entry(id).or_insert(1) -= 1;
        }

        // Recycle the surface back to the swap chain.
        if let Some(surface) = self.locked_surfaces.remove(&id) {
            self.swap_chains
                .get(id)
                .expect("Should always have a SwapChain for a busy WebGLContext")
                .recycle_surface(surface);
        }

        let _ = self.webgl_threads.finished_rendering_to_context(id);
    }
}
