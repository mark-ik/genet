/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::rc::Rc;

use euclid::default::Size2D;
use log::debug;
use paint_api::rendering_context_core::RenderingContextCore;
use paint_api::{ExternalImageSource, WebRenderExternalImageApi};
use servo_canvas_traits::webgl::{WebGLContextId, WebGLThreads};
use surfman::chains::SwapChains;
use surfman::{Device, SurfaceTexture};
use webgl::webgl_thread::WebGLContextBusyMap;

use crate::webgl_external_image_lifecycle::WebGLExternalImageLocks;

/// Bridge between the webrender::ExternalImage callbacks and the WebGLThreads.
pub struct WebGLExternalImages {
    rendering_context: Rc<dyn RenderingContextCore>,
    locks: WebGLExternalImageLocks<SurfaceTexture>,
}

impl WebGLExternalImages {
    pub fn new(
        webgl_threads: WebGLThreads,
        rendering_context: Rc<dyn RenderingContextCore>,
        swap_chains: SwapChains<WebGLContextId, Device>,
        busy_webgl_context_map: WebGLContextBusyMap,
    ) -> Self {
        Self {
            rendering_context,
            locks: WebGLExternalImageLocks::new(
                webgl_threads,
                swap_chains,
                busy_webgl_context_map,
            ),
        }
    }

    fn lock_swap_chain(&mut self, id: WebGLContextId) -> Option<(u32, Size2D<i32>)> {
        debug!("... locking chain {:?}", id);
        let front_buffer = self.locks.lock_front_buffer(id)?;
        let gl = self
            .rendering_context
            .gl()
            .expect("GL external image path requires a GL-capable rendering context");
        let (surface_texture, gl_texture, size) = match gl.create_texture(front_buffer) {
            Some(texture) => texture,
            None => {
                self.locks.abort_lock(id, None);
                return None;
            },
        };
        self.locks.insert_locked_resource(id, surface_texture);

        Some((gl_texture, size))
    }

    fn unlock_swap_chain(&mut self, id: WebGLContextId) -> Option<()> {
        debug!("... unlocked chain {:?}", id);
        let gl = self
            .rendering_context
            .gl()
            .expect("GL external image path requires a GL-capable rendering context");
        self.locks
            .unlock(id, |locked_front_buffer| gl.destroy_texture(locked_front_buffer))
    }
}

impl WebRenderExternalImageApi for WebGLExternalImages {
    fn lock(&mut self, id: u64) -> (ExternalImageSource<'_>, Size2D<i32>) {
        let (texture_id, size) = self.lock_swap_chain(WebGLContextId(id)).unwrap_or_default();
        (ExternalImageSource::NativeTexture(texture_id), size)
    }

    fn unlock(&mut self, id: u64) {
        self.unlock_swap_chain(WebGLContextId(id));
    }
}
