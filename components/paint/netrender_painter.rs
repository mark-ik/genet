/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C3 stub — `NetrenderPainter` scaffold for the netrender-driven paint
//! subsystem. Replaces the WebRender wrapper that was deleted as the
//! final C2 cut.
//!
//! This file is a compile-clean scaffold: the `Paint` struct exposes
//! the surface `components/servo/` calls into, but the methods are
//! either no-ops, sensible defaults, or `unimplemented!` for actions
//! that genuinely require the netrender wiring. Real
//! display-list-to-`netrender::Scene` translation and the
//! `Renderer::render_with_compositor` driver come in the proper C3
//! follow-up.
//!
//! The shape this scaffold targets is documented in
//! [`docs/2026-05-05_serval_netrender_cut_plan.md`](../../docs/2026-05-05_serval_netrender_cut_plan.md)
//! § C3.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use embedder_traits::{InputEventId, InputEventResult, ShutdownState};
use paint_api::{PaintMessage, PaintProxy, WebRenderExternalImageIdManager};
use servo_base::generic_channel::RoutedReceiver;
use servo_base::id::WebViewId;

use crate::InitialPaintState;

/// Carried over from the WebRender era for source compatibility with
/// the `servo` crate's `pub use paint::WebRenderDebugOption` re-export.
/// The variants no longer correspond to anything; the scaffold ignores
/// them. Will be renamed/replaced when the real netrender debug
/// surface lands.
#[derive(Copy, Clone)]
pub enum WebRenderDebugOption {
    Profiler,
    TextureCacheDebug,
    RenderTargetDebug,
}

/// `Paint` is Servo's rendering subsystem. In the netrender-driven
/// world (post-C3) it owns one `netrender::Renderer` per
/// `RenderingContext`, lowers display lists to `netrender::Scene`s,
/// and drives `Renderer::render_with_compositor` against a
/// consumer-supplied `netrender_device::Compositor`.
///
/// This scaffold keeps the public method surface stable so the
/// `components/servo/` consumer compiles, but does not yet do any
/// rendering. Methods that would drive real frames are
/// `unimplemented!()`; methods that report state return defaults.
pub struct Paint {
    paint_proxy: PaintProxy,
    paint_receiver: RoutedReceiver<PaintMessage>,
    shutdown_state: Rc<Cell<ShutdownState>>,
    webrender_external_image_id_manager: WebRenderExternalImageIdManager,
    #[cfg(feature = "webgpu")]
    wgpu_image_map: webgpu::canvas_context::WebGpuExternalImageMap,
    #[cfg(feature = "webxr")]
    webxr_registry: Option<webxr_api::Registry>,
}

impl Paint {
    pub fn new(state: InitialPaintState) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            paint_proxy: state.paint_proxy,
            paint_receiver: state.receiver,
            shutdown_state: state.shutdown_state,
            webrender_external_image_id_manager: WebRenderExternalImageIdManager::default(),
            #[cfg(feature = "webgpu")]
            wgpu_image_map: Default::default(),
            #[cfg(feature = "webxr")]
            webxr_registry: None,
        }))
    }

    pub fn receiver(&self) -> &RoutedReceiver<PaintMessage> {
        &self.paint_receiver
    }

    /// Drain a batch of `PaintMessage`s. The C3 scaffold drops them on
    /// the floor; the proper follow-up routes them into display-list
    /// lowering and the netrender scene.
    pub fn handle_messages(&self, _messages: Vec<PaintMessage>) {
        // C3 scaffold: real handling lands with the netrender Scene
        // translator.
    }

    pub fn notify_input_event_handled(
        &mut self,
        _webview_id: WebViewId,
        _event_id: InputEventId,
        _result: InputEventResult,
    ) {
        // C3 scaffold: input event accounting is layered on top of the
        // real per-webview painter state, which doesn't exist yet.
    }

    pub fn perform_updates(&mut self) -> bool {
        false
    }

    pub fn webviews_needing_repaint(&self) -> Vec<WebViewId> {
        Vec::new()
    }

    pub fn finish_shutting_down(&mut self) {
        self.shutdown_state.set(ShutdownState::FinishedShuttingDown);
    }

    pub fn webrender_external_image_id_manager(&self) -> WebRenderExternalImageIdManager {
        self.webrender_external_image_id_manager.clone()
    }

    #[cfg(feature = "webgpu")]
    pub fn webgpu_image_map(&self) -> webgpu::canvas_context::WebGpuExternalImageMap {
        self.wgpu_image_map.clone()
    }

    #[cfg(feature = "webxr")]
    pub fn webxr_main_thread_registry(&self) -> webxr_api::Registry {
        // C3 scaffold: WebXR is feature-gated to off in default builds
        // (per the project's W3C-knockout strategy). When the feature
        // is enabled, the proper follow-up wires a real registry; for
        // now this returns whatever was stored, panicking if absent.
        self.webxr_registry
            .clone()
            .expect("WebXR registry not initialised in C3 scaffold")
    }

    /// Carried over from WebRender era. The scaffold ignores debug
    /// options; the real netrender debug surface is a separate cut.
    pub fn toggle_webrender_debug(&self, _option: WebRenderDebugOption) {}

    /// Ensures the internal paint proxy is reachable for tests / call
    /// sites that take `&Paint` and look up the proxy.
    pub fn paint_proxy(&self) -> &PaintProxy {
        &self.paint_proxy
    }
}
