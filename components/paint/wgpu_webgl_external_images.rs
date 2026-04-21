/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebGL external image handler for the wgpu rendering backend.
//!
//! Imports WebGL canvas surfaces from surfman swap chains into wgpu textures
//! via the wgpu-native-texture-interop bridge. This replaces the GL-based
//! `WebGLExternalImages` when WebRender uses the wgpu backend.

use log::debug;
use servo_wgpu_interop_adapter::SurfmanSurfaceImporter;
use servo_canvas_traits::webgl::{WebGLContextId, WebGLThreads};
use surfman::chains::SwapChains;
use surfman::{Device, Surface};
use webgl::webgl_thread::WebGLContextBusyMap;
use webrender::WgpuExternalImageHandler;

use crate::webgl_external_image_lifecycle::WebGLExternalImageLocks;

/// WebGL external image handler for the wgpu backend.
///
/// On `lock_wgpu()`, takes the WebGL front buffer from the swap chain, binds it
/// to a dedicated surfman GL context, and imports the GL framebuffer into a wgpu
/// texture via native interop (Vulkan external memory / Metal IOSurface / D3D12
/// shared texture).
pub struct WgpuWebGLExternalImages {
    locks: WebGLExternalImageLocks<Surface>,
    importer: SurfmanSurfaceImporter,
}

impl WgpuWebGLExternalImages {
    pub fn new(
        webgl_threads: WebGLThreads,
        swap_chains: SwapChains<WebGLContextId, Device>,
        busy_webgl_context_map: WebGLContextBusyMap,
        wgpu_device: wgpu::Device,
        wgpu_queue: wgpu::Queue,
    ) -> Result<Self, surfman::Error> {
        let importer = SurfmanSurfaceImporter::new(wgpu_device, wgpu_queue)?;

        Ok(Self {
            locks: WebGLExternalImageLocks::new(
                webgl_threads,
                swap_chains,
                busy_webgl_context_map,
            ),
            importer,
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

        // Take the front buffer from the WebGL swap chain.
        let front_buffer = self.locks.lock_front_buffer(id)?;

        let imported = match self.importer.import_surface_default(front_buffer) {
            Ok(imported) => imported,
            Err(failure) => {
                let (error, surface) = failure.into_parts();
                log::error!("Failed to import WebGL texture into wgpu: {:?}", error);
                self.locks.abort_lock(id, surface);
                return None;
            },
        };

        self.locks.insert_locked_resource(id, imported.surface);

        let uv = webrender_api::units::TexelRect::new(
            0.0,
            0.0,
            imported.imported_texture.size.width as f32,
            imported.imported_texture.size.height as f32,
        );

        Some(webrender::WgpuExternalImage {
            texture: imported.imported_texture.texture,
            width: imported.imported_texture.size.width,
            height: imported.imported_texture.size.height,
            uv,
        })
    }

    fn unlock_wgpu(&mut self, key: webrender_api::ExternalImageId, _channel_index: u8) {
        let id = WebGLContextId(key.0);
        debug!("WgpuWebGLExternalImages: unlocking {:?}", id);
        let _ = self.locks.unlock(id, Some);
    }
}
