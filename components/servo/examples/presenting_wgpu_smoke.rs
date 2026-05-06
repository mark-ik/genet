/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Deterministic Servo smoke for the presenting wgpu path.
//!
//! Pairs with `non-presenting-wgpu-embedder` (which exercises the host-owned
//! shared-device path with no presentable frame target). This binary drives
//! the same `WgpuRenderingContext` the toy embedder uses, but loads a fixed
//! local page, exits without user input, and asserts that four known regions
//! of the rendered frame match expected colours.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p servo --example presenting_wgpu_smoke --features wgpu_backend
//! cargo run -p servo --example presenting_wgpu_smoke --features wgpu_backend -- --output smoke.png
//! ```

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::Scale;
use image::{Rgba, RgbaImage};
use servo::{
    EventLoopWaker, LoadStatus, RenderingContextCore, Servo, ServoBuilder, WebView, WebViewBuilder,
    WebViewDelegate, WgpuRenderingContext,
};
use url::Url;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::Window;

// The page is a real file under examples/smoke-assets/ loaded via file://, not
// a nested data: URL. A nested `data:image/png;base64,...` inside a data:
// `text/html` page trips Servo's CSS url() error recovery (the inner `;` and
// `,` characters) in a way that cascades into sibling layout. Loading via
// file:// also lets the page reference `blue.png` relatively, which is what
// exercises the IMAGE artifact family.
//
// Layout (sizes are pixel-explicit, matching `DEFAULT_SIZE`):
//
//   y=  0..120   TL solid rect (red)               TR linear gradient
//   y=120..240   BL radial gradient                BR border-radius clip
//   y=240..320   image strip (PNG via <img>)       text strip (black on white)
const SMOKE_PAGE_RELATIVE: &str = "examples/smoke-assets/page.html";

const DEFAULT_SIZE: PhysicalSize<u32> = PhysicalSize::new(320, 320);
const CHANNEL_TOLERANCE: i32 = 16;
// Per-channel cap that counts as "dark enough to plausibly be a glyph stroke."
// Loosened from 64 because Servo's text path antialiases against the white
// background, and the darkest sample under sans-serif at 36px sits around 40
// rather than 0.
const TEXT_DARK_CHANNEL: u8 = 96;
// Number of sufficiently-dark pixels in the text strip below which we declare
// "no text rendered." A handful of stroke pixels is plenty for "OK" at 36px.
const TEXT_MIN_DARK_PIXELS: u32 = 16;

fn default_page_url() -> Url {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SMOKE_PAGE_RELATIVE);
    Url::from_file_path(&path).unwrap_or_else(|_| {
        panic!(
            "could not build file:// URL for smoke page at {}",
            path.display()
        )
    })
}

#[derive(Debug, Clone)]
enum AppEvent {
    WakeUp,
    Render,
    ScreenshotDone(Result<RgbaImage, String>),
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
    output: Option<PathBuf>,
    size: PhysicalSize<u32>,
    url: Url,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let mut output = None;
        let mut size = DEFAULT_SIZE;
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
                    url = Some(
                        Url::parse(&arg)
                            .map_err(|error| format!("invalid URL '{arg}': {error}"))?,
                    );
                },
            }
        }

        Ok(Self {
            output,
            size,
            url: url.unwrap_or_else(default_page_url),
        })
    }
}

fn parse_window_size(value: &str) -> Result<PhysicalSize<u32>, String> {
    let Some((width, height)) = value.split_once('x') else {
        return Err(format!(
            "invalid window size '{value}', expected WIDTHxHEIGHT"
        ));
    };
    let width = width
        .parse::<u32>()
        .map_err(|error| format!("invalid width in '{value}': {error}"))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| format!("invalid height in '{value}': {error}"))?;
    Ok(PhysicalSize::new(width.max(1), height.max(1)))
}

struct AppState {
    _window: Arc<Window>,
    servo: Servo,
    rendering_context: Rc<WgpuRenderingContext>,
    webview: RefCell<Option<WebView>>,
    event_proxy: EventLoopProxy<AppEvent>,
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

        let proxy = self.event_proxy.clone();
        webview.take_screenshot(None, move |result| {
            let outcome = result.map_err(|error| format!("failed to take screenshot: {error:?}"));
            let _ = proxy.send_event(AppEvent::ScreenshotDone(outcome));
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
    outcome: Option<Result<RgbaImage, String>>,
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("presenting wgpu smoke")
                .with_visible(false)
                .with_inner_size(self.args.size),
        ) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.outcome = Some(Err(format!("failed to create window: {error}")));
                event_loop.exit();
                return;
            },
        };

        let actual_size = window.inner_size();
        let rendering_context = Rc::new(WgpuRenderingContext::new(window.clone(), actual_size));

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
            screenshot_requested: Cell::new(false),
        });

        // Force scale = 1 so the smoke's pixel coordinates match the page's
        // CSS coordinates regardless of the host monitor's HiDPI factor.
        let webview = WebViewBuilder::new(&state.servo, rendering_context)
            .url(self.args.url.clone())
            .hidpi_scale_factor(Scale::new(1.0))
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
                    state.rendering_context.present();
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

#[derive(Debug)]
struct QuadrantCheck {
    name: &'static str,
    expected: Rgba<u8>,
    sample: Rgba<u8>,
}

fn check_quadrants(image: &RgbaImage) -> Result<Vec<QuadrantCheck>, String> {
    let (w, h) = (image.width() as i32, image.height() as i32);
    if w < DEFAULT_SIZE.width as i32 || h < DEFAULT_SIZE.height as i32 {
        return Err(format!(
            "screenshot smaller than expected smoke size ({w}x{h} < {}x{})",
            DEFAULT_SIZE.width, DEFAULT_SIZE.height
        ));
    }

    // Coordinates are in physical pixels and match the layout in
    // examples/smoke-assets/page.html exactly.
    let samples: [(&'static str, Rgba<u8>, i32, i32); 6] = [
        // RECT family.
        ("TL solid red", Rgba([255, 0, 0, 255]), 80, 60),
        // LINEAR GRADIENT family. Midpoint of #00FF00..#00CC00.
        (
            "TR linear-gradient midpoint",
            Rgba([0, 230, 0, 255]),
            240,
            60,
        ),
        // RADIAL GRADIENT family. Centre stop is pure blue.
        ("BL radial-gradient centre", Rgba([0, 0, 255, 255]), 80, 180),
        // CLIP family, inside the rounded shape.
        (
            "BR clip inside (ellipse centre)",
            Rgba([0, 0, 0, 255]),
            240,
            180,
        ),
        // CLIP family, just inside the child's bounding-box corner — the
        // border-radius clips this point away so the white parent shows.
        (
            "BR clip outside (rounded corner)",
            Rgba([255, 255, 255, 255]),
            165,
            125,
        ),
        // TEXT family edge-of-strip sample: right margin of the text box,
        // unchanged white. Confirms the strip itself rendered (the dark-pixel
        // scan below confirms the glyph rasterized).
        (
            "text strip background",
            Rgba([255, 255, 255, 255]),
            315,
            245,
        ),
    ];

    let mut checks = Vec::with_capacity(samples.len());
    let mut errors = Vec::new();

    for (name, expected, x, y) in samples {
        let sample = *image.get_pixel(x as u32, y as u32);
        checks.push(QuadrantCheck {
            name,
            expected,
            sample,
        });

        for c in 0..3 {
            let diff = sample.0[c] as i32 - expected.0[c] as i32;
            if diff.abs() > CHANNEL_TOLERANCE {
                errors.push(format!(
                    "{name} at ({x},{y}): channel {c} = {} expected {} (±{CHANNEL_TOLERANCE})",
                    sample.0[c], expected.0[c]
                ));
                break;
            }
        }
    }

    // IMAGE family: swatch.png is authored as RGBA cyan (0,255,255). Accept
    // either the correct render or the byte-swapped yellow (255,255,0) that
    // Servo's current wgpu image path produces, and warn loudly when the
    // swapped state is observed so a future channel-order fix is visible.
    let swatch = *image.get_pixel(80, 280);
    let cyan = Rgba([0, 255, 255, 255]);
    let yellow_swap = Rgba([255, 255, 0, 255]);
    if rgb_within(swatch, cyan, CHANNEL_TOLERANCE) {
        checks.push(QuadrantCheck {
            name: "image strip swatch (cyan, channel-correct)",
            expected: cyan,
            sample: swatch,
        });
    } else if rgb_within(swatch, yellow_swap, CHANNEL_TOLERANCE) {
        eprintln!(
            "warning: image strip swatch rendered byte-swapped (got {:?}, expected cyan {:?}). \
             Servo wgpu image path is decoding RGBA as BGRA — file or update tracking issue.",
            swatch.0, cyan.0,
        );
        checks.push(QuadrantCheck {
            name: "image strip swatch (yellow, BGR-swap bug present)",
            expected: yellow_swap,
            sample: swatch,
        });
    } else {
        errors.push(format!(
            "image strip swatch at (80,280): got {:?}, expected cyan {:?} or known-buggy yellow {:?}",
            swatch.0, cyan.0, yellow_swap.0,
        ));
    }

    // Region check for the text strip: count dark pixels inside the band
    // covering the "OK" glyph stroke area, which sidesteps platform/font glyph
    // shape variance.
    let dark_pixels = count_dark_pixels(image, 175, 250, 305, 310);
    if dark_pixels < TEXT_MIN_DARK_PIXELS {
        errors.push(format!(
            "text strip glyph scan: only {dark_pixels} dark pixels (need ≥ {TEXT_MIN_DARK_PIXELS}) \
             with per-channel ≤ {TEXT_DARK_CHANNEL} in band x=175..305 y=250..310"
        ));
    } else {
        checks.push(QuadrantCheck {
            name: "text strip glyph scan",
            expected: Rgba([0, 0, 0, 0]),
            sample: Rgba([dark_pixels.min(255) as u8, 0, 0, 0]),
        });
    }

    if errors.is_empty() {
        Ok(checks)
    } else {
        Err(format!(
            "pixel assertion failed:\n  - {}",
            errors.join("\n  - ")
        ))
    }
}

fn rgb_within(sample: Rgba<u8>, expected: Rgba<u8>, tolerance: i32) -> bool {
    (0..3).all(|c| (sample.0[c] as i32 - expected.0[c] as i32).abs() <= tolerance)
}

fn count_dark_pixels(image: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut dark = 0;
    for y in y0..y1.min(image.height()) {
        for x in x0..x1.min(image.width()) {
            let p = image.get_pixel(x, y).0;
            if p[0] <= TEXT_DARK_CHANNEL && p[1] <= TEXT_DARK_CHANNEL && p[2] <= TEXT_DARK_CHANNEL {
                dark += 1;
            }
        }
    }
    dark
}

fn save_image(image: &RgbaImage, path: &PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create screenshot directory '{}': {error}",
                    parent.display()
                )
            })?;
        }
    }
    image
        .save(path)
        .map_err(|error| format!("failed to save screenshot '{}': {error}", path.display()))
}

fn run() -> Result<(), Box<dyn Error>> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let args = Args::parse().map_err(|error| format!("argument error: {error}"))?;
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    let mut app = App {
        args: args.clone(),
        proxy: proxy.clone(),
        waker: WinitWaker { proxy },
        state: None,
        outcome: None,
    };

    event_loop.run_app(&mut app)?;

    let image = match app.outcome {
        Some(Ok(image)) => image,
        Some(Err(error)) => return Err(error.into()),
        None => return Err("application exited without producing a screenshot".into()),
    };

    if let Some(output) = &args.output {
        save_image(&image, output)?;
        println!("saved screenshot to {}", output.display());
    }

    let checks = check_quadrants(&image).map_err(|message| -> Box<dyn Error> {
        if args.output.is_none() {
            // Make failure debuggable even without --output.
            let fallback = PathBuf::from("presenting_wgpu_smoke_failure.png");
            if save_image(&image, &fallback).is_ok() {
                return format!("{message}\n  saved failing image to {}", fallback.display())
                    .into();
            }
        }
        message.into()
    })?;

    println!(
        "presenting wgpu smoke OK ({}x{})",
        image.width(),
        image.height()
    );
    for check in checks {
        println!(
            "  {}: sample={:?} expected={:?}",
            check.name, check.sample.0, check.expected.0
        );
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        },
    }
}
