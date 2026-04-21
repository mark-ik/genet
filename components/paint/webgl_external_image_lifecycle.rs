/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use rustc_hash::FxHashMap;
use servo_canvas_traits::webgl::{WebGLContextId, WebGLThreads};
use surfman::chains::{SwapChainAPI, SwapChains, SwapChainsAPI};
use surfman::{Device, Surface};
use webgl::webgl_thread::WebGLContextBusyMap;

pub(crate) struct WebGLExternalImageLocks<T> {
    webgl_threads: WebGLThreads,
    swap_chains: SwapChains<WebGLContextId, Device>,
    busy_webgl_context_map: WebGLContextBusyMap,
    locked_resources: FxHashMap<WebGLContextId, T>,
}

impl<T> WebGLExternalImageLocks<T> {
    pub(crate) fn new(
        webgl_threads: WebGLThreads,
        swap_chains: SwapChains<WebGLContextId, Device>,
        busy_webgl_context_map: WebGLContextBusyMap,
    ) -> Self {
        Self {
            webgl_threads,
            swap_chains,
            busy_webgl_context_map,
            locked_resources: FxHashMap::default(),
        }
    }

    pub(crate) fn lock_front_buffer(&mut self, id: WebGLContextId) -> Option<Surface> {
        self.increment_busy(id);
        let front_buffer = self.swap_chains.get(id)?.take_surface();
        if front_buffer.is_none() {
            self.finish_rendering(id, None);
        }
        front_buffer
    }

    pub(crate) fn insert_locked_resource(&mut self, id: WebGLContextId, resource: T) {
        self.locked_resources.insert(id, resource);
    }

    pub(crate) fn abort_lock(&mut self, id: WebGLContextId, surface: Option<Surface>) {
        self.finish_rendering(id, surface);
    }

    pub(crate) fn unlock(
        &mut self,
        id: WebGLContextId,
        into_surface: impl FnOnce(T) -> Option<Surface>,
    ) -> Option<()> {
        let resource = self.locked_resources.remove(&id)?;
        self.finish_rendering(id, into_surface(resource));
        Some(())
    }

    fn finish_rendering(&mut self, id: WebGLContextId, surface: Option<Surface>) {
        self.decrement_busy(id);
        if let Some(surface) = surface {
            self.swap_chains
                .get(id)
                .expect("Should always have a SwapChain for a busy WebGLContext")
                .recycle_surface(surface);
        }
        let _ = self.webgl_threads.finished_rendering_to_context(id);
    }

    fn increment_busy(&self, id: WebGLContextId) {
        let mut busy = self.busy_webgl_context_map.write();
        *busy.entry(id).or_default() += 1;
    }

    fn decrement_busy(&self, id: WebGLContextId) {
        let mut busy = self.busy_webgl_context_map.write();
        *busy.entry(id).or_insert(1) -= 1;
    }
}