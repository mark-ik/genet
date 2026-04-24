/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase A validation embedder: the smallest thing that can drive a
//! Servo webview using only `RenderingContextCore + WgpuCapability`.
//!
//! No Surfman. No GL. No graphshell-specific plumbing. If this file
//! compiles, links, and paints a page against the split traits, the
//! wgpu-first design is genuinely wgpu-first.
//!
//! See `docs/2026-04-18_phase_a_toy_embedder.md` for the full rationale
//! and verification commands.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p servo --example toy_wgpu_embedder --features wgpu_backend
//! ```
//!
//! Expected: a winit window opens, Servo loads a page, content renders
//! through the wgpu compositor path. `cargo tree --example
//! toy_wgpu_embedder | grep surfman` should return empty-ish (Servo
//! itself still pulls Surfman for WebGL/XR producer paths; the embedder
//! does not).

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;

use euclid::Scale;
use servo::{
    RenderingContextCore, Servo, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate,
    WgpuRenderingContext,
};
use url::Url;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::Window;

#[derive(Debug, Clone, Copy)]
struct WakerEvent;

#[derive(Clone)]
struct Waker(EventLoopProxy<WakerEvent>);

impl Waker {
    fn new(event_loop: &EventLoop<WakerEvent>) -> Self {
        Self(event_loop.create_proxy())
    }
}

impl servo::EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self.0.send_event(WakerEvent);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let event_loop = EventLoop::<WakerEvent>::with_user_event().build()?;
    let mut app = App::Initial(Waker::new(&event_loop));
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct AppState {
    window: Arc<Window>,
    servo: Servo,
    rendering_context: Rc<WgpuRenderingContext>,
    webviews: RefCell<Vec<WebView>>,
}

impl WebViewDelegate for AppState {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.window.request_redraw();
    }
}

enum App {
    Initial(Waker),
    Running(Rc<AppState>),
}

impl ApplicationHandler<WakerEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Self::Initial(waker) = self else { return };

        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("toy wgpu embedder"))
                .expect("winit window"),
        );

        // ── the point of the whole file ─────────────────────────────────
        // `WgpuRenderingContext` implements `RenderingContextCore +
        // WgpuCapability` and nothing else. No GL. No Surfman.
        let size = window.inner_size();
        let rendering_context = Rc::new(WgpuRenderingContext::new(window.clone(), size));

        // Capability check: the new contract asserts we're wgpu-first.
        assert!(
            rendering_context.wgpu().is_some(),
            "WgpuRenderingContext must expose WgpuCapability",
        );
        assert!(
            rendering_context.gl().is_none(),
            "WgpuRenderingContext must NOT expose GlCapability",
        );
        // ────────────────────────────────────────────────────────────────

        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(waker.clone()))
            .build();
        servo.setup_logging();

        let state = Rc::new(AppState {
            window: window.clone(),
            servo,
            rendering_context: rendering_context.clone(),
            webviews: RefCell::new(Vec::new()),
        });

        let url = Url::parse("https://servo.org").expect("url");
        let webview = WebViewBuilder::new(&state.servo, rendering_context)
            .url(url)
            .hidpi_scale_factor(Scale::new(window.scale_factor() as f32))
            .delegate(state.clone())
            .build();
        state.webviews.borrow_mut().push(webview);

        *self = Self::Running(state);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: WakerEvent) {
        if let Self::Running(state) = self {
            state.servo.spin_event_loop();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Self::Running(state) = self else { return };
        match event {
            WindowEvent::Resized(size) => {
                state.rendering_context.resize(size);
                for webview in state.webviews.borrow().iter() {
                    webview.resize(size);
                }
            },
            WindowEvent::CloseRequested => {
                // Servo cleans up through its Drop impl; just exit the loop.
                event_loop.exit();
            },
            WindowEvent::RedrawRequested => {
                for webview in state.webviews.borrow().iter() {
                    webview.render();
                }
            },
            _ => {},
        }
    }
}
