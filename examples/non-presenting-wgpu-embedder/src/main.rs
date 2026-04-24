/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A minimal Servo embedder that mirrors Graphshell's non-presenting wgpu shape.
//!
//! Unlike the normal `wgpu-embedder`, this context exposes a shared
//! `wgpu::Device`/`wgpu::Queue` but deliberately does not expose a presentable
//! frame target. That forces Servo/WebRender to render into its internal
//! composite texture, and `WebView::take_screenshot()` must read back through
//! the paint-layer fallback.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p non-presenting-wgpu-embedder -- --output out.png
//! cargo run -p non-presenting-wgpu-embedder -- --output out.png https://servo.org
//! ```

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;

use dpi::PhysicalSize;
use euclid::Scale;
use paint_api::rendering_context_core::{RenderingContextCore, WgpuCapability};
use servo::{EventLoopWaker, LoadStatus, Servo, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate};
use url::Url;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::Window;

const DEFAULT_URL: &str = "data:text/html,%3Cbody%20style%3D%22margin%3A0%3Bbackground%3Argb(12%2C34%2C56)%3B%22%3E%3Cdiv%20style%3D%22width%3A100vw%3Bheight%3A100vh%3Bbackground%3Argb(12%2C34%2C56)%3B%22%3E%3C%2Fdiv%3E%3C%2Fbody%3E";

#[derive(Debug, Clone)]
enum AppEvent {
    WakeUp,
    Render,
    ScreenshotDone(Result<PathBuf, String>),
    Fatal(String),
}

#[derive(Clone)]
struct WinitWaker {
    proxy: EventLoopProxy<AppEvent>,
}

impl EventLoopWaker for WinitWaker {
    fn wake(&self) {
        let _ = self.proxy.send_event(AppEvent::WakeUp);
    }

    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
}

#[derive(Clone)]
struct Args {
    output: PathBuf,
    size: PhysicalSize<u32>,
    url: Url,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let mut output = None;
        let mut size = PhysicalSize::new(320, 240);
        let mut url = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--output" | "-o" => {
                    let path = args
                        .next()
                        .ok_or_else(|| "missing value for --output".to_string())?;
                    output = Some(PathBuf::from(path));
                },
                "--window-size" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "missing value for --window-size".to_string())?;
                    size = parse_window_size(&value)?;
                },
                _ if arg.starts_with('-') => {
                    return Err(format!("unknown argument: {arg}"));
                },
                _ => {
                    if url.is_some() {
                        return Err("only one URL may be provided".to_string());
                    }
                    url = Some(Url::parse(&arg).map_err(|error| format!("invalid URL '{arg}': {error}"))?);
                },
            }
        }

        Ok(Self {
            output: output.unwrap_or_else(|| PathBuf::from("non_presenting_wgpu_embedder.png")),
            size,
            url: url.unwrap_or_else(|| Url::parse(DEFAULT_URL).expect("default data URL must parse")),
        })
    }
}

fn parse_window_size(value: &str) -> Result<PhysicalSize<u32>, String> {
    let Some((width, height)) = value.split_once('x') else {
        return Err(format!("invalid window size '{value}', expected WIDTHxHEIGHT"));
    };
    let width = width
        .parse::<u32>()
        .map_err(|error| format!("invalid width in '{value}': {error}"))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| format!("invalid height in '{value}': {error}"))?;
    Ok(PhysicalSize::new(width.max(1), height.max(1)))
}

struct NonPresentingWgpuContext {
    size: RefCell<PhysicalSize<u32>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl NonPresentingWgpuContext {
    fn new(size: PhysicalSize<u32>) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        }))?;

        let wanted_features = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
            | wgpu::Features::DUAL_SOURCE_BLENDING
            | wgpu::Features::TIMESTAMP_QUERY;
        let required_features = adapter.features() & wanted_features;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("non_presenting_wgpu_embedder"),
            required_features,
            required_limits: wgpu::Limits {
                max_inter_stage_shader_variables: 28,
                ..Default::default()
            },
            ..Default::default()
        }))?;

        Ok(Self {
            size: RefCell::new(size),
            device,
            queue,
        })
    }
}

impl RenderingContextCore for NonPresentingWgpuContext {
    fn size(&self) -> PhysicalSize<u32> {
        *self.size.borrow()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        *self.size.borrow_mut() = PhysicalSize::new(size.width.max(1), size.height.max(1));
    }

    fn present(&self) {}

    fn read_to_image(&self, _rect: servo::DeviceIntRect) -> Option<image::RgbaImage> {
        None
    }

    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        Some(self)
    }
}

impl WgpuCapability for NonPresentingWgpuContext {
    fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    fn acquire_frame_target(&self) -> Option<wgpu::TextureView> {
        None
    }
}

struct AppState {
    _window: Rc<Window>,
    servo: Servo,
    rendering_context: Rc<NonPresentingWgpuContext>,
    webview: RefCell<Option<WebView>>,
    event_proxy: EventLoopProxy<AppEvent>,
    screenshot_path: PathBuf,
    screenshot_requested: Cell<bool>,
}

impl WebViewDelegate for AppState {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        let _ = self.event_proxy.send_event(AppEvent::Render);
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if status != LoadStatus::Complete || self.screenshot_requested.replace(true) {
            return;
        }

        let output = self.screenshot_path.clone();
        let proxy = self.event_proxy.clone();
        webview.take_screenshot(None, move |result| {
            let saved = match result {
                Ok(image) => {
                    if let Some(parent) = output.parent() {
                        if let Err(error) = std::fs::create_dir_all(parent) {
                            Err(format!("failed to create screenshot directory '{}': {error}", parent.display()))
                        } else {
                            image
                                .save(&output)
                                .map(|_| output.clone())
                                .map_err(|error| format!("failed to save screenshot '{}': {error}", output.display()))
                        }
                    } else {
                        image
                            .save(&output)
                            .map(|_| output.clone())
                            .map_err(|error| format!("failed to save screenshot '{}': {error}", output.display()))
                    }
                },
                Err(error) => Err(format!("failed to take screenshot: {error:?}")),
            };

            let _ = proxy.send_event(AppEvent::ScreenshotDone(saved));
        });
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, backtrace: Option<String>) {
        let message = match backtrace {
            Some(backtrace) => format!("pipeline crashed: {reason}\n{backtrace}"),
            None => format!("pipeline crashed: {reason}"),
        };
        let _ = self.event_proxy.send_event(AppEvent::Fatal(message));
    }
}

struct App {
    args: Args,
    proxy: EventLoopProxy<AppEvent>,
    waker: WinitWaker,
    state: Option<Rc<AppState>>,
    outcome: Option<Result<PathBuf, String>>,
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("non-presenting wgpu embedder")
                .with_visible(false)
                .with_inner_size(self.args.size),
        ) {
            Ok(window) => Rc::new(window),
            Err(error) => {
                self.outcome = Some(Err(format!("failed to create window: {error}")));
                event_loop.exit();
                return;
            },
        };

        let rendering_context = match NonPresentingWgpuContext::new(self.args.size) {
            Ok(rendering_context) => Rc::new(rendering_context),
            Err(error) => {
                self.outcome = Some(Err(format!("failed to create non-presenting wgpu context: {error}")));
                event_loop.exit();
                return;
            },
        };

        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(self.waker.clone()))
            .build();
        servo.setup_logging();

        let state = Rc::new(AppState {
            _window: window.clone(),
            servo,
            rendering_context: rendering_context.clone(),
            webview: RefCell::new(None),
            event_proxy: self.proxy.clone(),
            screenshot_path: self.args.output.clone(),
            screenshot_requested: Cell::new(false),
        });

        let webview = WebViewBuilder::new(&state.servo, rendering_context)
            .url(self.args.url.clone())
            .hidpi_scale_factor(Scale::new(window.scale_factor() as f32))
            .delegate(state.clone())
            .build();
        *state.webview.borrow_mut() = Some(webview);

        self.state = Some(state);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        let Some(state) = self.state.as_ref() else {
            return;
        };

        match event {
            AppEvent::WakeUp => state.servo.spin_event_loop(),
            AppEvent::Render => {
                if let Some(webview) = state.webview.borrow().clone() {
                    webview.render();
                }
            },
            AppEvent::ScreenshotDone(result) => {
                self.outcome = Some(result);
                event_loop.exit();
            },
            AppEvent::Fatal(message) => {
                self.outcome = Some(Err(message));
                event_loop.exit();
            },
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };

        match event {
            WindowEvent::Resized(size) => {
                state.rendering_context.resize(size);
                if let Some(webview) = state.webview.borrow().clone() {
                    webview.resize(size);
                }
            },
            WindowEvent::CloseRequested => {
                self.outcome = Some(Err("window closed before screenshot completed".to_string()));
                event_loop.exit();
            },
            _ => {},
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let args = Args::parse().map_err(|error| format!("argument error: {error}"))?;
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    let mut app = App {
        args,
        proxy: proxy.clone(),
        waker: WinitWaker { proxy },
        state: None,
        outcome: None,
    };

    event_loop.run_app(&mut app)?;

    match app.outcome {
        Some(Ok(path)) => {
            println!("saved screenshot to {}", path.display());
            Ok(())
        },
        Some(Err(error)) => Err(error.into()),
        None => Err("application exited without producing a screenshot".into()),
    }
}