/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `NetrenderPainter`. Routes inbound `PaintMessage`s into per-
//! pipeline `netrender::Scene`s built by the PaintList translator
//! ([`crate::translator`]).
//!
//! [`Paint::handle_messages`] consumes `PaintMessage::SendPaintList`,
//! walks the carried `PaintEnvelope` through `translate_envelope`,
//! and stores the resulting `Scene` keyed by `PipelineId`.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

use dpi::PhysicalSize;
use embedder_traits::{
    InputEventAndId, InputEventId, InputEventResult, ScreenshotCaptureError, Scroll, ShutdownState,
    ViewportDetails, WebViewPoint, WebViewRect,
};
use euclid::Scale;
use image::RgbaImage;
use log::warn;
use netrender::Scene;
use paint_api::display_list::PaintDisplayListInfo;
use paint_api::rendering_context_core::RenderingContextCore;
use paint_api::{PaintMessage, PaintProxy, WebRenderExternalImageIdManager, WebViewTrait};
use paint_types::PipelineId;
use paint_types::units::{DevicePixel, DevicePoint};
use rustc_hash::FxHashMap;
use servo_base::generic_channel::RoutedReceiver;
use servo_base::id::{PainterId, WebViewId};
use servo_geometry::DeviceIndependentPixel;
use style_traits::CSSPixel;

use crate::InitialPaintState;
use crate::compositor::{PaintCompositor, WgpuMasterCaptureBackend};
use crate::translator::{ExternalTextureDraw, translate_envelope_with_external_textures};

mod test_support;

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

/// Per-pipeline painter state. One entry per known `PipelineId`,
/// allocated lazily on the first `SendPaintList` for that pipeline.
pub struct PipelineState {
    /// Latest translated scene. Replaced wholesale on each
    /// `SendPaintList` (no incremental update yet).
    pub scene: Scene,
    /// Layout-side metadata bundle. Carries the scroll-tree, epoch,
    /// caret property binding, and root reference frame id.
    pub paint_info: PaintDisplayListInfo,
    /// Same-device external textures that should be overlaid into the
    /// rendered frame after the ordinary Scene paints.
    pub(crate) external_textures: Vec<ExternalTextureDraw>,
}

/// `Paint` is Servo's rendering subsystem. It owns one
/// `netrender::Renderer` per `RenderingContext`, lowers paint
/// envelopes to `netrender::Scene`s via [`crate::translator`], and
/// drives `Renderer::render_with_compositor` against a
/// consumer-supplied `netrender_device::Compositor`.
/// Per-webview painter state. Tracks zoom, hidpi, and the registered
/// `WebViewTrait` handle (used to push events back to the embedder).
struct WebViewState {
    view: Box<dyn WebViewTrait>,
    viewport_details: ViewportDetails,
    page_zoom: f32,
    pinch_zoom: f32,
    hidpi_scale: Scale<f32, DeviceIndependentPixel, DevicePixel>,
    hidden: bool,
}

pub struct Paint {
    paint_proxy: PaintProxy,
    paint_receiver: RoutedReceiver<PaintMessage>,
    shutdown_state: Rc<Cell<ShutdownState>>,
    webrender_external_image_id_manager: WebRenderExternalImageIdManager,
    /// Per-pipeline painter state. `RefCell` lets `handle_messages`
    /// mutate via `&self` (the existing call signature, which the
    /// scaffold preserved when the embedder loop took `&Paint`).
    pipelines: RefCell<FxHashMap<PipelineId, PipelineState>>,
    /// `WebViewId`s whose pipelines have a pending Scene to render in
    /// the next frame. Drained by `webviews_needing_repaint` so the
    /// embedder knows where to call `composite`.
    dirty_webviews: RefCell<Vec<WebViewId>>,
    /// Per-webview state. Allocated on `add_webview` / removed on
    /// `remove_webview`.
    webviews: RefCell<FxHashMap<WebViewId, WebViewState>>,
    /// Embedder-provided rendering contexts, keyed by the painter id
    /// allocated in `register_rendering_context`.
    rendering_contexts: RefCell<FxHashMap<PainterId, Rc<dyn RenderingContextCore>>>,
    /// Painter id counter; new ids handed out in `register_rendering_context`.
    next_painter_id: Cell<u32>,
    /// `netrender::Renderer` per registered rendering context. Built on
    /// `register_rendering_context` from the context's `WgpuCapability`
    /// (instance + adapter + device + queue). `Renderer` is `!Send`,
    /// matching the rest of `Paint`'s `Rc<RefCell<...>>` shape — the
    /// painter is single-threaded.
    renderers: RefCell<FxHashMap<PainterId, netrender::Renderer>>,
    /// `WebViewId` → `PipelineId` map populated on `SendPaintList`.
    /// `Paint::render` looks up the latest scene by walking
    /// webview → pipeline → `pipelines[pipeline]`.
    webview_to_pipeline: RefCell<FxHashMap<WebViewId, PipelineId>>,
    /// Per-pipeline compositor. Defaults to
    /// [`WgpuMasterCaptureBackend`] (the wgpu-shared-device embedder
    /// route — captures the master so [`Paint::composite_texture`]
    /// can hand it back). Embedders that present pixels directly to
    /// the OS compositor install a per-platform backend (e.g.
    /// [`crate::WindowsDxgiBackend`]) via
    /// [`Paint::install_compositor`].
    compositor: RefCell<Box<dyn PaintCompositor>>,
    /// Same-device producer textures keyed by display-list external
    /// texture items. GPU handles stay in-process; display lists carry
    /// only stable keys.
    external_textures: RefCell<FxHashMap<u64, wgpu::Texture>>,
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

    pub fn receiver(&self) -> &RoutedReceiver<PaintMessage> {
        &self.paint_receiver
    }

    /// Drain a batch of `PaintMessage`s. `SendPaintList` routes
    /// through the translator and stores the resulting Scene +
    /// metadata under the message's pipeline id; other variants
    /// route to the renderer / compositor / image-update flows.
    pub fn handle_messages(&self, messages: Vec<PaintMessage>) {
        for message in messages {
            self.handle_one_message(message);
        }
    }

    fn handle_one_message(&self, message: PaintMessage) {
        match message {
            // Pipeline_id is read from paint_info (the envelope itself
            // doesn't carry one — keeping it engine-agnostic).
            PaintMessage::SendPaintList {
                webview_id,
                envelope,
                paint_info,
            } => {
                let pipeline_id = paint_info.pipeline_id;
                let translated = translate_envelope_with_external_textures(&envelope);
                self.pipelines.borrow_mut().insert(
                    pipeline_id,
                    PipelineState {
                        scene: translated.scene,
                        paint_info,
                        external_textures: translated.external_textures,
                    },
                );
                self.webview_to_pipeline
                    .borrow_mut()
                    .insert(webview_id, pipeline_id);
                let mut dirty = self.dirty_webviews.borrow_mut();
                if !dirty.contains(&webview_id) {
                    dirty.push(webview_id);
                }
            },
            PaintMessage::PipelineExited(_, pipeline_id, _) => {
                let key: PipelineId = pipeline_id.into();
                self.pipelines.borrow_mut().remove(&key);
            },
            PaintMessage::GenerateFrame(_) => {
                // C4 territory: rendering a frame needs a
                // `netrender::Renderer` handle and a `Compositor`
                // impl; the painter doesn't hold either yet.
            },
            // Everything else is wire-format pass-through that the
            // C4 cut will hook into the renderer state. Logging for
            // now so silent drops are visible during bring-up.
            other => {
                let tag: &'static str = (&other).into();
                warn!("[netrender painter] unhandled PaintMessage::{tag} (C4 territory)");
            },
        }
    }

    /// Read the latest Scene for a given pipeline, if any. Used by
    /// tests and the renderer-driver in C4.
    pub fn pipeline_scene(&self, pipeline_id: PipelineId) -> Option<Ref<'_, Scene>> {
        let borrow = self.pipelines.borrow();
        if borrow.contains_key(&pipeline_id) {
            Some(Ref::map(borrow, |m| &m.get(&pipeline_id).unwrap().scene))
        } else {
            None
        }
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
        std::mem::take(&mut *self.dirty_webviews.borrow_mut())
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

    // -------------------------------------------------------------------
    // C4 Paint surface — webview / rendering-context lifecycle
    // -------------------------------------------------------------------

    /// Register an embedder-provided rendering context with the
    /// painter. Returns a fresh `PainterId` keyed on the registered
    /// context — the embedder later uses this id to query the
    /// composite texture (`composite_texture`).
    ///
    /// If the context exposes a [`WgpuCapability`], a
    /// [`netrender::Renderer`] is built against the same wgpu device
    /// and stored under the new painter id. This is what `Paint::render`
    /// drives via `Renderer::render_with_compositor` — the entire C4
    /// rendering path hangs off this construction.
    pub fn register_rendering_context(
        &self,
        rendering_context: Rc<dyn RenderingContextCore>,
    ) -> PainterId {
        let id = PainterId::next();
        if let Some(wgpu_cap) = rendering_context.wgpu() {
            let handles = netrender::WgpuHandles {
                instance: wgpu_cap.instance(),
                adapter: wgpu_cap.adapter(),
                device: wgpu_cap.device(),
                queue: wgpu_cap.queue(),
            };
            let options = netrender::NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            };
            match netrender::create_netrender_instance(handles, options) {
                Ok(renderer) => {
                    self.renderers.borrow_mut().insert(id, renderer);
                },
                Err(err) => {
                    warn!(
                        "[netrender painter] create_netrender_instance failed: {err:?}; \
                         render() will be a no-op for painter {id:?}"
                    );
                },
            }
        }
        self.rendering_contexts
            .borrow_mut()
            .insert(id, rendering_context);
        id
    }

    /// Resize the rendering context attached to the given webview.
    /// First-cut: looks up the webview's rendering context (if any)
    /// and forwards the resize.
    pub fn resize_rendering_context(&self, webview_id: WebViewId, size: PhysicalSize<u32>) {
        if let Some(rc) = self
            .rendering_contexts
            .borrow()
            .get(&PainterId::from(webview_id))
        {
            rc.resize(size);
        }
    }

    /// Add a webview handle to the painter. Stored under the supplied
    /// id for later input / scroll / zoom / show / hide operations.
    pub fn add_webview(&self, view: Box<dyn WebViewTrait>, viewport_details: ViewportDetails) {
        let id = view.id();
        let hidpi_scale = Scale::new(viewport_details.hidpi_scale_factor.0);
        self.webviews.borrow_mut().insert(
            id,
            WebViewState {
                view,
                viewport_details,
                page_zoom: 1.0,
                pinch_zoom: 1.0,
                hidpi_scale,
                hidden: false,
            },
        );
    }

    /// Drop painter-side state for a webview. Idempotent on unknown
    /// ids.
    pub fn remove_webview(&self, webview_id: WebViewId) {
        self.webviews.borrow_mut().remove(&webview_id);
        if let Some(state) = self.webviews.borrow().get(&webview_id) {
            let _ = state.view.id();
        }
    }

    // -------------------------------------------------------------------
    // C4 Paint surface — frame lifecycle
    // -------------------------------------------------------------------

    /// Drive a render for the given webview. Looks up the latest
    /// translated `Scene` for the webview's pipeline, hands it to
    /// `netrender::Renderer::render_with_compositor`, and lets the
    /// `WgpuMasterCaptureBackend` stash the master texture for
    /// [`Paint::composite_texture`].
    ///
    /// No-op if any of: the webview has no pipeline yet (no
    /// `SendPaintList` received), the painter id has no registered
    /// rendering context, or the rendering context didn't expose a
    /// `WgpuCapability` at registration time (so no Renderer was
    /// built). The C4 milestone leaves per-platform OS handoff to a
    /// follow-up; the WgpuMasterCaptureBackend satisfies the trait shape and
    /// makes the master texture readable end-to-end.
    pub fn render(&self, webview_id: WebViewId) {
        let painter_id = PainterId::from(webview_id);

        let pipeline_id = match self.webview_to_pipeline.borrow().get(&webview_id) {
            Some(&id) => id,
            None => return,
        };

        let mut renderers = self.renderers.borrow_mut();
        let renderer = match renderers.get_mut(&painter_id) {
            Some(r) => r,
            None => return,
        };

        let pipelines = self.pipelines.borrow();
        let state = match pipelines.get(&pipeline_id) {
            Some(state) => state,
            None => return,
        };
        let scene = &state.scene;
        let registered_external_textures = self.external_textures.borrow();
        let mut external_views = Vec::new();
        for external in &state.external_textures {
            let Some(texture) = registered_external_textures.get(&external.texture_key) else {
                continue;
            };
            external_views.push((
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
                external.placement,
                external.scene_op_boundary,
            ));
        }
        let external_composites: Vec<_> = external_views
            .iter()
            .map(|(view, placement, scene_op_boundary)| {
                netrender::ExternalTextureComposite::new(view, *placement)
                    .with_scene_op_boundary(*scene_op_boundary)
            })
            .collect();

        let mut compositor = self.compositor.borrow_mut();
        // Double-deref past the RefMut + Box to get
        // `&mut dyn PaintCompositor`, then upcast to
        // `&mut dyn netrender_device::compositor::Compositor` (the
        // trait `render_with_compositor` accepts). Trait upcasting
        // stable since rustc 1.86.
        let pc: &mut dyn PaintCompositor = &mut **compositor;
        if external_composites.is_empty() {
            renderer.render_with_compositor(
                scene,
                wgpu::TextureFormat::Rgba8Unorm,
                pc,
                netrender::peniko::Color::TRANSPARENT,
            );
        } else {
            renderer.render_with_compositor_and_external_textures(
                scene,
                wgpu::TextureFormat::Rgba8Unorm,
                pc,
                netrender::peniko::Color::TRANSPARENT,
                &external_composites,
            );
        }
    }

    /// Hand back the most recently presented composite texture for the
    /// given painter. `None` until a frame has actually rendered, or
    /// `None` when the installed compositor is a per-platform OS
    /// handoff backend (those present to the OS compositor and don't
    /// expose a wgpu texture back to the embedder).
    ///
    /// Available with the wgpu-shared-device embedder route — i.e.
    /// when the default [`WgpuMasterCaptureBackend`] (or another
    /// backend whose [`PaintCompositor::last_master`] returns `Some`)
    /// is installed.
    pub fn composite_texture(&self, _painter_id: PainterId) -> Option<wgpu::Texture> {
        self.compositor.borrow().last_master().cloned()
    }

    /// Replace the installed compositor with the embedder-supplied
    /// `boxed_compositor`. The embedder calls this once after
    /// constructing `Paint` to switch from the default
    /// [`WgpuMasterCaptureBackend`] to a per-platform OS-handoff
    /// backend (e.g. `ServoCompositor::new(host,
    /// WindowsDxgiBackend::new(host, hwnd)?)`).
    ///
    /// After installation, the previous compositor is dropped and any
    /// state it held is released. `composite_texture` returns
    /// whatever the new compositor reports via `last_master`.
    pub fn install_compositor(&self, boxed_compositor: Box<dyn PaintCompositor>) {
        *self.compositor.borrow_mut() = boxed_compositor;
    }

    /// Register or replace a same-device producer texture referenced
    /// by `ServalDisplayItem::ExternalTexture`.
    pub fn install_external_texture(&self, key: u64, texture: wgpu::Texture) {
        self.external_textures.borrow_mut().insert(key, texture);
    }

    /// Set the embedder-side hidpi scale for a webview.
    pub fn set_hidpi_scale_factor(
        &self,
        webview_id: WebViewId,
        scale: Scale<f32, DeviceIndependentPixel, DevicePixel>,
    ) {
        if let Some(state) = self.webviews.borrow_mut().get_mut(&webview_id) {
            state.hidpi_scale = scale;
            state.viewport_details.hidpi_scale_factor = Scale::new(scale.0);
        }
    }

    pub fn show_webview(&self, webview_id: WebViewId) -> Result<(), &'static str> {
        let mut webviews = self.webviews.borrow_mut();
        let state = webviews
            .get_mut(&webview_id)
            .ok_or("Paint::show_webview: unknown webview")?;
        state.hidden = false;
        Ok(())
    }

    pub fn hide_webview(&self, webview_id: WebViewId) -> Result<(), &'static str> {
        let mut webviews = self.webviews.borrow_mut();
        let state = webviews
            .get_mut(&webview_id)
            .ok_or("Paint::hide_webview: unknown webview")?;
        state.hidden = true;
        Ok(())
    }

    // -------------------------------------------------------------------
    // C4 Paint surface — input / scroll / zoom
    // -------------------------------------------------------------------

    pub fn notify_scroll_event(
        &self,
        _webview_id: WebViewId,
        _scroll: Scroll,
        _point: WebViewPoint,
    ) {
        // C4 stub: scroll-tree hit-testing + delta application lands
        // when the painter consumes `paint_info.scroll_tree` per
        // pipeline.
    }

    /// Hit-test an input event. Returns `true` if the event was
    /// dispatched to the embedder; `false` if the constellation
    /// should handle it.
    ///
    /// First-cut: returns `false` to signal "nothing handled" so the
    /// embedder's pending-event queue path takes over.
    pub fn notify_input_event(&self, _webview_id: WebViewId, _event: InputEventAndId) -> bool {
        false
    }

    pub fn set_page_zoom(&self, webview_id: WebViewId, new_zoom: f32) {
        if let Some(state) = self.webviews.borrow_mut().get_mut(&webview_id) {
            state.page_zoom = new_zoom.clamp(0.1, 10.0);
        }
    }

    pub fn page_zoom(&self, webview_id: WebViewId) -> f32 {
        self.webviews
            .borrow()
            .get(&webview_id)
            .map(|s| s.page_zoom)
            .unwrap_or(1.0)
    }

    pub fn adjust_pinch_zoom(
        &self,
        webview_id: WebViewId,
        pinch_zoom_delta: f32,
        _center: DevicePoint,
    ) {
        if let Some(state) = self.webviews.borrow_mut().get_mut(&webview_id) {
            state.pinch_zoom = (state.pinch_zoom * pinch_zoom_delta).clamp(1.0, 10.0);
        }
    }

    pub fn pinch_zoom(&self, webview_id: WebViewId) -> f32 {
        self.webviews
            .borrow()
            .get(&webview_id)
            .map(|s| s.pinch_zoom)
            .unwrap_or(1.0)
    }

    pub fn device_pixels_per_page_pixel(
        &self,
        webview_id: WebViewId,
    ) -> Scale<f32, CSSPixel, DevicePixel> {
        let state = self.webviews.borrow();
        let zoom = state.get(&webview_id).map(|s| s.page_zoom).unwrap_or(1.0);
        let hidpi = state
            .get(&webview_id)
            .map(|s| s.hidpi_scale.0)
            .unwrap_or(1.0);
        Scale::new(zoom * hidpi)
    }

    // -------------------------------------------------------------------
    // C4 Paint surface — debug / capture
    // -------------------------------------------------------------------

    pub fn capture_webrender(&self, _webview_id: WebViewId) {
        // C4 stub: webrender's debug capture surface is gone with the
        // C2 cut; netrender has its own profiling layer that the
        // capture call would route to in a follow-up.
    }

    /// Asynchronously take a screenshot of the webview's rendering
    /// context. C4 stub: invokes the callback synchronously with
    /// `Err(InvalidWebView)`; real screenshot capture lands when the
    /// renderer wiring is complete.
    pub fn request_screenshot(
        &self,
        _webview_id: WebViewId,
        _rect: Option<WebViewRect>,
        callback: Box<dyn FnOnce(Result<RgbaImage, ScreenshotCaptureError>)>,
    ) {
        callback(Err(ScreenshotCaptureError::WebViewDoesNotExist));
    }
}
