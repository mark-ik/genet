// A pure-wgpu Servo embedder using the zero-copy render_to_view() path.
//
// Uses WgpuRenderingContext from servo-paint-api which owns the wgpu device
// and surface. Servo/WebRender renders directly into the surface texture
// via render_to_view() — no intermediate blit needed.
//
// Features:
//   - Zero-copy rendering: WebRender writes directly to the swap-chain frame
//   - sRGB colour fix: handled by WgpuRenderingContext's non-sRGB view
//   - Proper resize: webview.resize() propagates through the context
//   - HiDPI: scale factor propagated on ScaleFactorChanged
//   - Mouse: move, click, wheel, leave-viewport all routed to Servo
//   - Keyboard: full key translation via keyutils module
//   - Cursor: window cursor icon updated from Servo's cursor change callbacks
//   - Title: window title tracks the loaded page title
//   - Shutdown: clean servo.deinit() on window close
//
// Usage:
//   cargo run -p wgpu-embedder -- <url>

mod keyutils;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::Scale;
use log::info;
use paint_api::rendering_context::RenderingContext;
use paint_api::wgpu_rendering_context::WgpuRenderingContext;
use servo::{
    Cursor, DevicePoint, EventLoopWaker, InputEvent,
    MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent,
    WheelMode,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

// Pixels per "line" for line-delta scroll events.
const LINE_HEIGHT: f32 = 76.0;
const LINE_WIDTH: f32 = 76.0;

// ---------------------------------------------------------------------------
// Delegate
// ---------------------------------------------------------------------------

struct EmbedderDelegate {
    frame_ready: Arc<AtomicBool>,
    event_proxy: EventLoopProxy<AppEvent>,
}

#[derive(Debug)]
enum AppEvent {
    WakeUp,
    CursorChanged(CursorIcon),
    TitleChanged(Option<String>),
}

impl WebViewDelegate for EmbedderDelegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.render();
        self.frame_ready.store(true, Ordering::Relaxed);
    }

    fn notify_cursor_changed(&self, _webview: WebView, cursor: Cursor) {
        let icon = servo_cursor_to_winit(cursor);
        let _ = self.event_proxy.send_event(AppEvent::CursorChanged(icon));
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let _ = self.event_proxy.send_event(AppEvent::TitleChanged(title));
    }
}

fn servo_cursor_to_winit(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Move => CursorIcon::Move,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::None => CursorIcon::Default,
    }
}

// ---------------------------------------------------------------------------
// EventLoopWaker
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    waker: Box<dyn EventLoopWaker>,
    proxy: EventLoopProxy<AppEvent>,
    url: String,
    state: Option<RunningState>,
    frame_ready: Arc<AtomicBool>,
}

struct RunningState {
    window: Arc<Window>,
    servo: servo::Servo,
    webview: WebView,
    // Input tracking
    cursor_pos: Cell<DevicePoint>,
    cursor_in_window: Cell<bool>,
    modifiers: Cell<ModifiersState>,
    hidpi_scale: f64,
}

impl App {
    fn new(proxy: EventLoopProxy<AppEvent>, waker: Box<dyn EventLoopWaker>, url: String) -> Self {
        Self {
            waker,
            proxy,
            url,
            state: None,
            frame_ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("wgpu-embedder (Servo)")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1024u32, 768)),
                )
                .expect("Failed to create window"),
        );

        let win_size = window.inner_size();
        let hidpi_scale = window.scale_factor();

        // WgpuRenderingContext handles device/queue/surface creation, sRGB config,
        // and frame acquisition. Servo/WebRender renders directly into surface
        // textures via render_to_view().
        let rendering_context: Rc<dyn RenderingContext> = Rc::new(
            WgpuRenderingContext::new(
                window.clone(),
                PhysicalSize::new(win_size.width, win_size.height),
            ),
        );

        info!("Created WgpuRenderingContext ({}x{})", win_size.width, win_size.height);

        let servo = ServoBuilder::default()
            .event_loop_waker(self.waker.clone())
            .build();
        servo.setup_logging();

        let url = servo::ServoUrl::parse(&self.url)
            .unwrap_or_else(|_| servo::ServoUrl::parse("about:blank").unwrap());
        let webview = WebViewBuilder::new(&servo, rendering_context)
            .url(url.as_url().clone())
            .hidpi_scale_factor(Scale::new(hidpi_scale as f32))
            .delegate(Rc::new(EmbedderDelegate {
                frame_ready: self.frame_ready.clone(),
                event_proxy: self.proxy.clone(),
            }))
            .build();

        self.state = Some(RunningState {
            window,
            servo,
            webview,
            cursor_pos: Cell::new(DevicePoint::zero()),
            cursor_in_window: Cell::new(false),
            modifiers: Cell::new(ModifiersState::empty()),
            hidpi_scale,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            // ---- Window lifecycle ----
            WindowEvent::CloseRequested => {
                // Drop the state (and servo) before exiting.
                self.state = None;
                event_loop.exit();
            }

            // ---- Resize ----
            WindowEvent::Resized(new_size) => {
                state.webview.resize(new_size);
                state.window.request_redraw();
            }

            // ---- HiDPI scale change ----
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.hidpi_scale = scale_factor;
                state.webview.set_hidpi_scale_factor(Scale::new(scale_factor as f32));
            }

            // ---- Redraw ----
            WindowEvent::RedrawRequested => {
                // With render_to_view(), Servo/WebRender renders directly into
                // the surface texture during webview.render(). The frame is
                // presented by the RenderingContext automatically. We just need
                // to trigger a new frame.
                state.window.request_redraw();
            }

            // ---- Mouse ----
            WindowEvent::CursorMoved { position, .. } => {
                let pt = DevicePoint::new(position.x as f32, position.y as f32);
                state.cursor_pos.set(pt);
                state.cursor_in_window.set(true);
                state.webview.notify_input_event(InputEvent::MouseMove(
                    MouseMoveEvent::new(pt.into()),
                ));
            }
            WindowEvent::CursorLeft { .. } => {
                state.cursor_in_window.set(false);
                state
                    .webview
                    .notify_input_event(InputEvent::MouseLeftViewport(
                        MouseLeftViewportEvent::default(),
                    ));
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                let point = state.cursor_pos.get();
                let servo_button = winit_button_to_servo(button);
                let action = match btn_state {
                    ElementState::Pressed => MouseButtonAction::Down,
                    ElementState::Released => MouseButtonAction::Up,
                };
                state.webview.notify_input_event(InputEvent::MouseButton(
                    MouseButtonEvent::new(action, servo_button, point.into()),
                ));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let point = state.cursor_pos.get();
                let (dx, dy, mode) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        ((x * LINE_WIDTH) as f64, (y * LINE_HEIGHT) as f64, WheelMode::DeltaPixel)
                    }
                    MouseScrollDelta::PixelDelta(d) => (d.x, d.y, WheelMode::DeltaPixel),
                };
                state.webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: dx, y: dy, z: 0.0, mode },
                    point.into(),
                )));
            }

            // ---- Keyboard ----
            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers.set(mods.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let mods = state.modifiers.get();
                let keyboard_event = keyutils::keyboard_event_from_winit(&event, mods);
                state
                    .webview
                    .notify_input_event(InputEvent::Keyboard(keyboard_event));
            }

            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        let Some(state) = &self.state else {
            return;
        };
        match event {
            AppEvent::WakeUp => {
                state.servo.spin_event_loop();
                if self.frame_ready.swap(false, Ordering::Relaxed) {
                    state.window.request_redraw();
                }
            }
            AppEvent::CursorChanged(icon) => {
                state.window.set_cursor(icon);
            }
            AppEvent::TitleChanged(title) => {
                let t = title.unwrap_or_else(|| "wgpu-embedder (Servo)".to_string());
                state.window.set_title(&t);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.servo.spin_event_loop();
            if self.frame_ready.swap(false, Ordering::Relaxed) {
                state.window.request_redraw();
            }
        }
    }
}

fn winit_button_to_servo(button: MouseButton) -> ServoMouseButton {
    match button {
        MouseButton::Left => ServoMouseButton::Left,
        MouseButton::Right => ServoMouseButton::Right,
        MouseButton::Middle => ServoMouseButton::Middle,
        MouseButton::Back => ServoMouseButton::Back,
        MouseButton::Forward => ServoMouseButton::Forward,
        MouseButton::Other(v) => ServoMouseButton::Other(v),
    }
}

fn main() {
    // SAFETY: called before any threads are spawned.
    unsafe { std::env::set_var("SERVO_WGPU_BACKEND", "1") };

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://servo.org".to_string());

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let waker = Box::new(WinitWaker { proxy: proxy.clone() });

    let mut app = App::new(proxy, waker, url);
    event_loop.run_app(&mut app).expect("Event loop failed");
}
