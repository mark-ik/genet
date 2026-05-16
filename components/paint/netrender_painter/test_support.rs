/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

impl Paint {
    /// Test-only constructor — wires up minimal stub `PaintProxy` /
    /// `RoutedReceiver` / `ShutdownState` so integration tests can
    /// drive `handle_messages` + `render` + `composite_texture`
    /// without standing up the full `InitialPaintState` (which needs
    /// time + memory profiler chans, constellation senders, a real
    /// `EventLoopWaker`, etc.).
    ///
    /// Production embedders construct via [`Paint::new`] with the
    /// real `InitialPaintState`. This shortcut exists for the
    /// 5.5b "Paint::render driven by a test" done-condition probe
    /// and isn't intended for production use.
    pub fn new_for_test() -> Rc<RefCell<Self>> {
        use embedder_traits::EventLoopWaker;

        struct NoopWaker;
        impl EventLoopWaker for NoopWaker {
            fn clone_box(&self) -> Box<dyn EventLoopWaker> {
                Box::new(NoopWaker)
            }
            fn wake(&self) {}
        }

        // RoutedReceiver<T> is a type alias for
        // crossbeam_channel::Receiver<Result<T, ipc_channel::IpcError>>,
        // so the unbounded() Sender / Receiver pair already matches
        // the PaintProxy + Paint shape — no wrapping needed.
        let (sender, receiver) = crossbeam_channel::unbounded();
        let cross_process_paint_api = paint_api::CrossProcessPaintApi::dummy();
        let paint_proxy = PaintProxy {
            sender,
            cross_process_paint_api,
            event_loop_waker: Box::new(NoopWaker),
        };
        let paint_receiver: RoutedReceiver<PaintMessage> = receiver;
        let shutdown_state = Rc::new(Cell::new(embedder_traits::ShutdownState::NotShuttingDown));

        Rc::new(RefCell::new(Self {
            paint_proxy,
            paint_receiver,
            shutdown_state,
            webrender_external_image_id_manager: WebRenderExternalImageIdManager::default(),
            pipelines: RefCell::new(FxHashMap::default()),
            dirty_webviews: RefCell::new(Vec::new()),
            webviews: RefCell::new(FxHashMap::default()),
            rendering_contexts: RefCell::new(FxHashMap::default()),
            next_painter_id: Cell::new(0),
            renderers: RefCell::new(FxHashMap::default()),
            webview_to_pipeline: RefCell::new(FxHashMap::default()),
            compositor: RefCell::new(Box::new(WgpuMasterCaptureBackend::new())),
            external_textures: RefCell::new(FxHashMap::default()),
            #[cfg(feature = "webgpu")]
            wgpu_image_map: Default::default(),
            #[cfg(feature = "webxr")]
            webxr_registry: None,
        }))
    }

    /// Install a pre-built `netrender::Renderer` under `painter_id`,
    /// bypassing the [`Self::register_rendering_context`] flow that
    /// constructs one from a `RenderingContext`'s `WgpuCapability`.
    ///
    /// Intended for **integration tests** that drive `Paint::render`
    /// without a real rendering context — the test boots wgpu via
    /// `netrender::boot()` (or builds the handles itself), constructs
    /// the Renderer directly, and installs it under a known
    /// `PainterId`. Production embedders use `register_rendering_context`.
    pub fn install_renderer(&self, painter_id: PainterId, renderer: netrender::Renderer) {
        self.renderers.borrow_mut().insert(painter_id, renderer);
    }
}
