# Phase A — Toy Embedder Validation Artifact

**Companion to**:

- [`2026-04-18_servo_wgpuification_plan.md`](2026-04-18_servo_wgpuification_plan.md) — plan (updated with multi-embedder DoD)
- [`2026-04-18_phase_a_rendering_context_audit.md`](2026-04-18_phase_a_rendering_context_audit.md) — consumer audit
- [`2026-04-18_phase_a_trait_design.md`](2026-04-18_phase_a_trait_design.md) — trait design spec

## Purpose

The updated Phase A done-gate requires three independent embedders to compile against the split traits: `graphshell`, `servoshell`, and a **toy embedder in ~200 lines**. The toy embedder exists to prove the trait design is **actually wgpu-first**, not graphshell-shaped.

Test shape:

- A ~200-line embedder using **only** `RenderingContextCore + WgpuCapability` (no `GlCapability`, no Surfman dependency) should compile and drive a minimal webview.
- A ~200-line embedder using **only** `RenderingContextCore` (no capabilities) should *not* compile — it has no way to actually render anything. This asymmetry is the test.

If the toy embedder compiles, the trait design has passed the "not secretly GL-dependent" test.

This file contains:

1. **The toy embedder itself** (~200 lines) — as a ready-to-land Rust file once Phase A's traits are in place.
2. **How to use it** — command to build; expected output.
3. **What it doesn't exercise** — items explicitly out of scope for this artifact, with pointers to where they live.

## The toy embedder

Drop this at `components/servo/examples/toy_wgpu_embedder.rs` once `RenderingContextCore` + `WgpuCapability` land. Until then it lives here as a design artifact.

```rust
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase A validation embedder: the smallest thing that can drive a
//! Servo webview using only `RenderingContextCore + WgpuCapability`.
//!
//! No Surfman. No GL. No graphshell-specific plumbing. If this file
//! compiles against the split traits, the wgpu-first design is
//! genuinely wgpu-first.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use dpi::PhysicalSize;
use euclid::Scale;
use servo::{Servo, ServoBuilder, WebView, WebViewBuilder};
use servo_paint_api::{RenderingContextCore, WgpuCapability, WindowHandles};
use url::Url;
use webrender_api::units::DeviceIntRect;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

// ---------------------------------------------------------------------------
// ToyWgpuContext: a minimal RenderingContextCore implementation
// ---------------------------------------------------------------------------

struct ToyWgpuContext {
    window: Rc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: RefCell<wgpu::SurfaceConfiguration>,
}

impl ToyWgpuContext {
    fn new(window: Rc<Window>) -> Result<Rc<Self>, Box<dyn Error>> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        // SAFETY: we own `window` via Rc and guarantee it outlives the surface.
        let surface = instance.create_surface(window.clone())?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
        }))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(),
            None,
        ))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps.formats[0];
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Ok(Rc::new(Self {
            window,
            instance,
            surface,
            device,
            queue,
            config: RefCell::new(config),
        }))
    }
}

impl RenderingContextCore for ToyWgpuContext {
    fn size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        let mut config = self.config.borrow_mut();
        config.width = size.width.max(1);
        config.height = size.height.max(1);
        self.surface.configure(&self.device, &config);
    }

    fn present(&self) {
        // Frame presentation happens implicitly when the acquired
        // texture is dropped; we don't need to call anything here
        // because `SurfaceTexture::present` is invoked by the caller
        // that acquired the target.
    }

    fn read_to_image(&self, _rect: DeviceIntRect) -> Option<image::RgbaImage> {
        // Not implemented for toy embedder. Real wgpu readback uses
        // a staging buffer + map_read; out of scope here.
        None
    }

    fn window_handles(&self) -> Option<WindowHandles> {
        Some(WindowHandles {
            window: self.window.window_handle().ok()?.as_raw(),
            display: self.window.display_handle().ok()?.as_raw(),
        })
    }

    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        Some(self)
    }

    // gl() uses the default `None` — toy embedder has no GL path.
}

impl WgpuCapability for ToyWgpuContext {
    fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    fn acquire_frame_target(&self) -> Option<wgpu::TextureView> {
        let frame = self.surface.get_current_texture().ok()?;
        Some(frame.texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }
}

// ---------------------------------------------------------------------------
// Minimal winit application driving Servo against ToyWgpuContext
// ---------------------------------------------------------------------------

struct AppState {
    window: Rc<Window>,
    servo: Servo,
    rendering_context: Rc<ToyWgpuContext>,
    webview: RefCell<Option<WebView>>,
}

impl servo::WebViewDelegate for AppState {
    fn notify_new_frame_ready(&self, _: WebView) {
        self.window.request_redraw();
    }
}

enum App {
    Initial,
    Running(Rc<AppState>),
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self, Self::Initial) {
            return;
        }
        let window = Rc::new(
            event_loop
                .create_window(Window::default_attributes())
                .expect("winit window"),
        );
        let rendering_context = ToyWgpuContext::new(window.clone())
            .expect("toy wgpu rendering context");

        let servo = ServoBuilder::default().build();
        servo.setup_logging();

        let state = Rc::new(AppState {
            window: window.clone(),
            servo,
            rendering_context: rendering_context.clone(),
            webview: RefCell::new(None),
        });

        let url = Url::parse("https://servo.org").expect("url");
        let webview = WebViewBuilder::new(&state.servo, rendering_context)
            .url(url)
            .hidpi_scale_factor(Scale::new(window.scale_factor() as f32))
            .delegate(state.clone())
            .build();
        *state.webview.borrow_mut() = Some(webview);

        *self = Self::Running(state);
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Self::Running(state) = self else { return };
        match event {
            WindowEvent::Resized(size) => state.rendering_context.resize(size),
            WindowEvent::CloseRequested => state.servo.start_shutdown(),
            WindowEvent::RedrawRequested => {
                if let Some(webview) = state.webview.borrow().as_ref() {
                    webview.paint();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let event_loop = EventLoop::new()?;
    let mut app = App::Initial;
    event_loop.run_app(&mut app)?;
    Ok(())
}
```

Line count (rough): 180 lines of Rust, ~200 with blank lines and the module doc header. Fits the budget.

## How to use it (once Phase A lands)

```sh
cd servo-wgpu
cargo run --example toy_wgpu_embedder
```

Expected: a winit window opens, Servo loads `servo.org`, page renders through the wgpu compositor path. No Surfman in the dependency tree for this binary. No `gleam` or `glow` usage.

Build-time validation: `cargo tree --example toy_wgpu_embedder | grep surfman` should return nothing.

## What the toy embedder proves

1. **The trait split actually works without GL.** If this compiles, the core + wgpu capability is sufficient to drive a webview. Any `unreachable!()` panics on `gleam_gl_api`/`glow_gl_api` have been eliminated — not just hidden by caller discipline.
2. **`window_handles()` is the right abstraction for surface creation.** The embedder constructs a wgpu surface from `window.window_handle()` + `window.display_handle()` directly, bundled through `WindowHandles`. No Surfman `Connection` required.
3. **`acquire_frame_target()` replaces `prepare_for_rendering` cleanly on the wgpu path.** The embedder never calls `prepare_for_rendering`; the compositor calls `acquire_frame_target` when it needs the target view.
4. **`Option<&dyn WgpuCapability>` is ergonomic enough.** The impl returns `Some(self)` from `wgpu()`; consumers destructure `Some(wgpu) = ctx.wgpu()` and hold `&dyn WgpuCapability`. Standard pattern, no lifetime gymnastics.
5. **Multi-embedder claim is concrete.** graphshell + servoshell + toy embedder = three independent consumers of the trait, each with its own shape. If a change to the trait breaks the toy, that's early warning before production embedders notice.

## What the toy embedder doesn't exercise (and where those live)

| Concern | Where |
|---|---|
| GL path (Surfman, make_current, gleam_gl_api) | `servoshell`, `graphshell` today; WebGL producer longer-term |
| `read_to_image` readback | Noted as `TODO` in `WgpuRenderingContext`; separate follow-on ticket |
| Real `refresh_driver` integration | Default timer-based driver is fine for toy purposes |
| Input event translation (keyboard, pointer) | Not needed for "does the page paint?" validation |
| Multi-webview / tabbing | servoshell covers this |
| IME / accessibility | out of scope |
| Servo process model (content processes etc.) | Servo itself handles this; embedder doesn't need to know |

Deliberate omissions — the point is the minimum surface that proves the trait design, not a reimplementation of servoshell.

## Acceptance criteria for this artifact

- [ ] File compiles at `components/servo/examples/toy_wgpu_embedder.rs` against the Phase A split traits
- [ ] Runs and loads `servo.org` on Windows (primary target for the branch's wgpu/DX12 work)
- [ ] `cargo tree --example toy_wgpu_embedder | grep surfman` returns empty
- [ ] Line count stays under ~220 lines including blank lines and module docs
- [ ] Any future addition that requires `GlCapability` to make the toy compile is a signal that `RenderingContextCore + WgpuCapability` isn't actually sufficient, and the trait design should be revisited before shipping.

## Maintenance note

This file is a *design validation artifact*, not a supported embedder. It lives in `examples/` because it's runnable; it should not accumulate features over time. If the toy grows past ~250 lines, it's no longer a toy and its value as a minimality proof is gone. Promote concrete features to servoshell or a new demo crate, and keep the toy stripped-down.
