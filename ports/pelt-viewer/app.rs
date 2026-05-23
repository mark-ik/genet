/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Xilem viewer app.
//!
//! A nav bar over a `WebContent` widget. The widget reserves its area as
//! a Masonry `External` layer (it paints nothing itself); the serval
//! content for that area is rendered on Masonry's **shared** wgpu device
//! and composited into the layer's bounds via `copy_texture_to_texture`
//! — no GPU→CPU readback. Resize falls out: the External bounds drive
//! the content render size.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dpi::LogicalSize;
use masonry::accesskit::{Node, Role};
use masonry::core::{
    AccessCtx, ChildrenIds, ErasedAction, LayoutCtx, MeasureCtx, PaintCtx, PaintLayerMode,
    PropertiesRef, RegisterCtx, Widget, WidgetId,
};
use masonry::imaging::Painter;
use masonry::kurbo::{Axis, Size};
use masonry::layout::{LenReq, Length};
use masonry::theme::default_property_set;
use masonry_winit::app::{
    AppDriver, DriverCtx, ExternalCompositeCtx, MasonryState, WgpuContext, WindowId, run_with,
};
use xilem::core::{MessageCtx, MessageResult, Mut, View, ViewMarker};
use xilem::view::{FlexExt, flex_col, flex_row, text_button, text_input};
use xilem::{EventLoop, Pod, ViewCtx, WidgetView, WindowOptions, Xilem};

use crate::render::build_scene;

const SAMPLE_HTML: &str = "<html><body>\
<h1>Pelt Viewer</h1>\
<p>This page is parsed, cascaded, laid out, and painted by \
<span class=\"hot\">serval</span>, then rendered through \
<span class=\"cool\">netrender</span> on Masonry's shared GPU device and \
composited zero-copy into a <span class=\"hot\">Xilem</span> window.</p>\
<div class=\"box\">A bordered block with its own background.</div>\
</body></html>";

const SAMPLE_CSS: &[&str] = &[
    "body { background-color: rgb(250, 250, 252); color: rgb(30, 30, 40); }",
    "h1 { color: rgb(20, 40, 90); }",
    ".hot { color: rgb(200, 40, 60); font-weight: bold; }",
    ".cool { color: rgb(30, 110, 170); font-weight: bold; }",
    ".box { background-color: rgb(230, 236, 245); border: 3px solid rgb(120, 140, 180); }",
];

/// The content to render, shared between the reactive UI (which sets it
/// on navigation) and the driver (which renders + composites it). The
/// `generation` bumps on every change so the driver knows to re-render.
struct RenderRequest {
    html: String,
    stylesheets: Vec<String>,
    base_dir: Option<PathBuf>,
    generation: u64,
}

/// Resolve a nav input into HTML + stylesheets + base dir: a readable
/// file path → its contents (base dir = its directory, no extra CSS);
/// otherwise the built-in sample page.
fn resolve(input: &str) -> (String, Vec<String>, Option<PathBuf>) {
    match std::fs::read_to_string(input) {
        Ok(contents) => {
            let base = Path::new(input).parent().map(Path::to_path_buf);
            (contents, Vec::new(), base)
        },
        Err(_) => (
            SAMPLE_HTML.to_string(),
            SAMPLE_CSS.iter().map(|s| s.to_string()).collect(),
            None,
        ),
    }
}

// ── Reactive UI ────────────────────────────────────────────────────────

struct AppState {
    nav_input: String,
    request: Arc<Mutex<RenderRequest>>,
}

fn app_logic(state: &mut AppState) -> impl WidgetView<AppState> + use<> {
    let nav_bar = flex_row((
        text_input(
            state.nav_input.clone(),
            |state: &mut AppState, new_text: String| {
                state.nav_input = new_text;
            },
        )
        .flex(1.0),
        text_button("Go", |state: &mut AppState| {
            let (html, stylesheets, base_dir) = resolve(&state.nav_input);
            let mut req = state.request.lock().unwrap();
            req.html = html;
            req.stylesheets = stylesheets;
            req.base_dir = base_dir;
            req.generation += 1;
        }),
    ));

    flex_col((nav_bar, web_content().flex(1.0)))
}

// ── WebContent widget: an External layer that paints nothing ─────────────

struct WebContentWidget;

impl Widget for WebContentWidget {
    type Action = ();

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        _axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        // No intrinsic content; take whatever space the flex parent
        // offers (flex(1.0) makes that the remaining content area).
        match len_req {
            LenReq::MinContent | LenReq::MaxContent => Length::const_px(0.0),
            LenReq::FitContent(space) => space,
        }
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, _size: Size) {}

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, _painter: &mut Painter<'_>) {
        // Reserve this widget's box as an externally-realized layer; the
        // serval content is composited into it by ServalDriver.
        ctx.set_paint_layer_mode(PaintLayerMode::External);
    }

    fn accessibility_role(&self) -> Role {
        Role::Image
    }

    fn accessibility(&mut self, _ctx: &mut AccessCtx<'_>, _props: &PropertiesRef<'_>, _node: &mut Node) {}

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }
}

/// A view whose widget reserves an `External` content layer.
struct WebContent;

fn web_content() -> WebContent {
    WebContent
}

impl ViewMarker for WebContent {}
impl<State: 'static> View<State, (), ViewCtx> for WebContent {
    type Element = Pod<WebContentWidget>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (ctx.create_pod(WebContentWidget), ())
    }

    fn rebuild(
        &self,
        _prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        _element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
    }

    fn teardown(&self, (): &mut Self::ViewState, _ctx: &mut ViewCtx, _element: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        (): &mut Self::ViewState,
        _message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        _app_state: &mut State,
    ) -> MessageResult<()> {
        MessageResult::Stale
    }
}

// ── Driver: shared device + zero-copy content composite ──────────────────

/// Wraps Xilem's driver, adding the GPU device-share (`on_wgpu_ready`)
/// and the External-layer composite. Standard `AppDriver` callbacks
/// delegate to the inner Xilem driver.
struct ServalDriver<D: AppDriver> {
    inner: D,
    request: Arc<Mutex<RenderRequest>>,
    gpu: Option<Gpu>,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: netrender::Renderer,
    /// Last content texture + the (generation, size) it was rendered for.
    cache: Option<CachedContent>,
}

struct CachedContent {
    generation: u64,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
}

impl<D: AppDriver> ServalDriver<D> {
    fn new(inner: D, request: Arc<Mutex<RenderRequest>>) -> Self {
        Self { inner, request, gpu: None }
    }
}

impl<D: AppDriver> AppDriver for ServalDriver<D> {
    fn on_action(
        &mut self,
        window_id: WindowId,
        ctx: &mut DriverCtx<'_, '_>,
        widget_id: WidgetId,
        action: ErasedAction,
    ) {
        self.inner.on_action(window_id, ctx, widget_id, action);
    }

    fn on_async_action(
        &mut self,
        window_id: WindowId,
        ctx: &mut DriverCtx<'_, '_>,
        action: ErasedAction,
    ) {
        self.inner.on_async_action(window_id, ctx, action);
    }

    fn on_start(&mut self, state: &mut MasonryState<'_>) {
        self.inner.on_start(state);
    }

    fn on_close_requested(&mut self, window_id: WindowId, ctx: &mut DriverCtx<'_, '_>) {
        self.inner.on_close_requested(window_id, ctx);
    }

    fn on_wgpu_ready(&mut self, wgpu: &WgpuContext<'_>) {
        let handles = netrender::WgpuHandles {
            instance: wgpu.instance.clone(),
            adapter: wgpu.adapter.clone(),
            device: wgpu.device.clone(),
            queue: wgpu.queue.clone(),
        };
        match netrender::create_netrender_instance(
            handles,
            netrender::NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            },
        ) {
            Ok(renderer) => {
                self.gpu = Some(Gpu {
                    device: wgpu.device.clone(),
                    queue: wgpu.queue.clone(),
                    renderer,
                    cache: None,
                });
            },
            Err(err) => eprintln!("[pelt-viewer] netrender init on shared device failed: {err:?}"),
        }
        self.inner.on_wgpu_ready(wgpu);
    }

    fn composite_external_layers(&mut self, ctx: &mut ExternalCompositeCtx<'_>) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let req = self.request.lock().unwrap();

        for layer in ctx.layers {
            let [x, y, w, h] = layer.bounds;
            if w == 0 || h == 0 {
                continue;
            }

            // Re-render only when the content or the size changed.
            let fresh = matches!(
                &gpu.cache,
                Some(c) if c.generation == req.generation && c.width == w && c.height == h
            );
            if !fresh {
                let sheets: Vec<&str> = req.stylesheets.iter().map(String::as_str).collect();
                let scene = build_scene(&req.html, &sheets, req.base_dir.as_deref(), w, h);
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("pelt-viewer content"),
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
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                gpu.renderer
                    .render_vello(&scene, &view, netrender::ColorLoad::default());
                gpu.cache = Some(CachedContent {
                    generation: req.generation,
                    width: w,
                    height: h,
                    texture,
                });
            }

            // Copy the content texture into the layer's bounds on the
            // shared surface target — same device, no readback.
            let content = &gpu.cache.as_ref().expect("just populated").texture;
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("pelt-viewer external composite"),
                });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: content,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: ctx.target_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            gpu.queue.submit([encoder.finish()]);
        }
    }
}

// ── Boot ─────────────────────────────────────────────────────────────────

/// Boot the viewer window. `initial` is the first nav input (a file
/// path, or anything that isn't a readable file → the sample page).
pub fn run(initial: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let initial = initial.unwrap_or_else(|| "sample".to_string());
    let (html, stylesheets, base_dir) = resolve(&initial);
    let request = Arc::new(Mutex::new(RenderRequest {
        html,
        stylesheets,
        base_dir,
        generation: 1,
    }));

    let app_state = AppState {
        nav_input: initial,
        request: Arc::clone(&request),
    };

    let window_options = WindowOptions::new("Pelt Viewer")
        .with_min_inner_size(LogicalSize::new(800.0, 648.0));

    let xilem = Xilem::new_simple(app_state, app_logic, window_options);

    let event_loop = EventLoop::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let (driver, windows) =
        xilem.into_driver_and_windows(move |event| proxy.send_event(event).map_err(|e| e.0));
    let serval = ServalDriver::new(driver, request);

    run_with(event_loop, windows, serval, default_property_set())?;
    Ok(())
}
