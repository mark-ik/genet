/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`WeldSurface`] — the host seam over weld's CEF accelerated-OSR producer —
//! and [`WeldProducer`], the adapter satisfying `inker::SurfaceProducer`.

use inker::{
    Cookie, CursorShape, FocusReason, KeyboardEvent, MouseEvent, NativeTextureHandle,
    NavigationEvent, PointerEvent, SurfaceError, SurfaceFrame, SurfaceProducer, SurfaceSettings,
    SurfaceSyncHandle, WebFeatureStatus, WebFrameTransportMode, WebMessage, WebSurface,
    WebSurfaceCapabilities, WebSurfaceEvent,
};

/// A frame produced by a [`WeldSurface`]: the shared GPU texture handle the host
/// imports, plus the import-once metadata that `inker::SurfaceFrame` omits.
pub struct WeldFrame {
    /// The platform shared-texture handle for the weld-owned copy CEF's
    /// `OnAcceleratedPaint` produced (Windows: a DX12 shared HANDLE; Linux: a
    /// DMA-BUF fd; macOS: an IOSurface ref). The host imports it on its own wgpu
    /// device. Never CEF's callback-scoped handle — weld copies into an owned
    /// resource inside the callback first.
    pub texture: NativeTextureHandle,
    pub sync: SurfaceSyncHandle,
    pub width: u32,
    pub height: u32,
    /// Monotonic generation of the owned shared allocation (from welding's
    /// `NativeFrame::generation`): bumps when weld (re)allocates (first frame /
    /// resize), constant while it overwrites the same allocation. Maps straight to
    /// `inker::SurfaceFrame::resource_epoch`, so the host's import cache re-imports
    /// only when it changes.
    pub resource_epoch: u64,
}

/// The host-implemented CEF composite: a `welding::CefRuntime` + a
/// `welding::CefSurfaceProducer` driving Chromium in accelerated OSR. This crate
/// cannot fabricate it (it does not depend on CEF), so it defines the seam and
/// the host wires it. The live `welding` calls each method maps to:
///
/// - [`resize`](WeldSurface::resize) → `CefSurfaceProducer` resize (CEF
///   `WasResized`).
/// - [`acquire_frame`](WeldSurface::acquire_frame) → drain the producer's
///   `PendingFrameSlot` latest-frame mailbox (Windows uses CEF's message-loop
///   thread; other platforms tick `CefRuntime::do_message_loop_work()` first).
/// - [`load_url`](WeldSurface::load_url) / [`load_html`](WeldSurface::load_html)
///   → CEF `LoadURL` / data URL.
/// - `notify_*` → CEF `SendMouseClickEvent` / `SendKeyEvent` / etc.
/// - `poll_*` → drained from the CEF client handlers the host registers.
///
/// **Subprocess tax:** building any `WeldSurface` requires the host to have
/// already called `welding::CefRuntime::execute_process_from` at the very top of
/// `main()` (CEF re-executes the host binary for its subprocesses). That cannot
/// live in this crate; it is a precondition of the host's `WeldProducerFactory`.
///
/// Not `Send`: CEF browser objects are thread-affine; the host drives one
/// surface per thread (the `inker::SurfaceProducer` contract).
pub trait WeldSurface {
    fn resize(&mut self, width: u32, height: u32) -> Result<(), SurfaceError>;

    /// Pump CEF and return the latest frame, if a new one is ready. `Ok(None)`
    /// when nothing new this tick.
    fn acquire_frame(&mut self) -> Result<Option<WeldFrame>, SurfaceError>;

    fn load_url(&mut self, url: &str) -> Result<(), SurfaceError>;
    fn load_html(&mut self, html: &str) -> Result<(), SurfaceError>;
    fn reload(&mut self) -> Result<(), SurfaceError>;
    fn stop(&mut self) -> Result<(), SurfaceError>;
    fn go_back(&mut self) -> Result<(), SurfaceError>;
    fn go_forward(&mut self) -> Result<(), SurfaceError>;
    fn can_go_back(&self) -> bool;
    fn can_go_forward(&self) -> bool;

    fn notify_mouse(&mut self, ev: MouseEvent) -> Result<(), SurfaceError>;
    fn notify_pointer(&mut self, ev: PointerEvent) -> Result<(), SurfaceError>;
    fn notify_keyboard(&mut self, ev: KeyboardEvent) -> Result<(), SurfaceError>;
    fn focus(&mut self, reason: FocusReason) -> Result<(), SurfaceError>;

    fn poll_navigation_event(&mut self) -> Option<NavigationEvent>;
    fn poll_cursor_shape(&mut self) -> Option<CursorShape>;
    fn poll_web_message(&mut self) -> Option<WebMessage>;

    fn web_capabilities(&self) -> WebSurfaceCapabilities {
        let mut caps = WebSurfaceCapabilities {
            backend_name: "weld.cef".into(),
            frame_transport: WebFrameTransportMode::ImportedTexture,
            ..WebSurfaceCapabilities::default()
        };
        caps.script.execute = WebFeatureStatus::Supported;
        caps.script.result = WebFeatureStatus::Partial {
            detail: "result-bearing script requires the host WeldSurface to wire CEF eval response plumbing".into(),
        };
        caps.devtools = WebFeatureStatus::Supported;
        caps.popups = WebFeatureStatus::Supported;
        caps.context_menus = WebFeatureStatus::Partial {
            detail: "CEF context-menu events require host-side client callback wiring".into(),
        };
        caps.auth = WebFeatureStatus::Partial {
            detail: "CEF auth events require host-side request callback wiring".into(),
        };
        caps.downloads = WebFeatureStatus::Partial {
            detail: "CEF download events require host-side download callback wiring".into(),
        };
        caps.snapshot = WebFeatureStatus::Partial {
            detail: "snapshots are available when the host WeldSurface implements capture".into(),
        };
        caps.degradation_reasons
            .push("weld-engine defaults to unsupported cookie/script-result controls until the host overrides them".into());
        caps
    }

    fn set_cookie(&mut self, _cookie: &Cookie) -> Result<(), SurfaceError> {
        Err(SurfaceError::Unsupported(
            "weld-engine cookie control is not wired yet".into(),
        ))
    }

    fn get_cookies_for_url(&mut self, _url: &str) -> Result<Vec<Cookie>, SurfaceError> {
        Err(SurfaceError::Unsupported(
            "weld-engine cookie reads are not wired yet".into(),
        ))
    }

    fn delete_cookie(&mut self, _cookie: &Cookie) -> Result<(), SurfaceError> {
        Err(SurfaceError::Unsupported(
            "weld-engine cookie delete is not wired yet".into(),
        ))
    }

    fn execute_script_with_result(&mut self, _script: &str) -> Result<String, SurfaceError> {
        Err(SurfaceError::Unsupported(
            "weld-engine script result control is not wired yet".into(),
        ))
    }

    fn apply_settings(&mut self, settings: &SurfaceSettings) -> Result<(), SurfaceError>;
    fn capture_snapshot_png(&mut self) -> Result<Vec<u8>, SurfaceError>;
}

/// Adapts a `Box<dyn WeldSurface>` onto `inker::SurfaceProducer`.
pub struct WeldProducer {
    inner: Box<dyn WeldSurface>,
}

impl WeldProducer {
    pub fn new(inner: Box<dyn WeldSurface>) -> Self {
        Self { inner }
    }
}

impl SurfaceProducer for WeldProducer {
    fn resize(&mut self, width: u32, height: u32) -> Result<(), SurfaceError> {
        self.inner.resize(width, height)
    }

    /// No-op: CEF renders offscreen and the host composites the imported texture
    /// at the tile rect, so there is no producer-side on-host visual to offset.
    fn set_offset(&mut self, _x: i32, _y: i32) -> Result<(), SurfaceError> {
        Ok(())
    }

    fn acquire_frame(&mut self) -> Result<Option<SurfaceFrame>, SurfaceError> {
        // WeldFrame maps 1:1 onto SurfaceFrame now that the contract carries
        // `resource_epoch` (the host's import cache reads it directly).
        Ok(self.inner.acquire_frame()?.map(|f| SurfaceFrame {
            texture: f.texture,
            sync: f.sync,
            width: f.width,
            height: f.height,
            resource_epoch: f.resource_epoch,
        }))
    }

    fn send_mouse_input(&mut self, ev: MouseEvent) -> Result<(), SurfaceError> {
        self.inner.notify_mouse(ev)
    }

    fn send_pointer_input(&mut self, ev: PointerEvent) -> Result<(), SurfaceError> {
        self.inner.notify_pointer(ev)
    }

    fn send_keyboard_input(&mut self, ev: KeyboardEvent) -> Result<(), SurfaceError> {
        self.inner.notify_keyboard(ev)
    }

    fn move_focus(&mut self, reason: FocusReason) -> Result<(), SurfaceError> {
        self.inner.focus(reason)
    }

    fn poll_cursor_shape(&mut self) -> Option<CursorShape> {
        self.inner.poll_cursor_shape()
    }

    fn apply_settings(&mut self, settings: &SurfaceSettings) -> Result<(), SurfaceError> {
        self.inner.apply_settings(settings)
    }

    fn capture_snapshot_png(&mut self) -> Result<Vec<u8>, SurfaceError> {
        self.inner.capture_snapshot_png()
    }

    fn as_web_surface(&mut self) -> Option<&mut dyn WebSurface> {
        Some(self)
    }
}

impl WebSurface for WeldProducer {
    fn capabilities(&self) -> WebSurfaceCapabilities {
        self.inner.web_capabilities()
    }

    fn navigate_to_url(&mut self, url: &str) -> Result<(), SurfaceError> {
        self.inner.load_url(url)
    }

    fn navigate_to_string(&mut self, html: &str) -> Result<(), SurfaceError> {
        self.inner.load_html(html)
    }

    fn reload(&mut self) -> Result<(), SurfaceError> {
        self.inner.reload()
    }

    fn stop(&mut self) -> Result<(), SurfaceError> {
        self.inner.stop()
    }

    fn go_back(&mut self) -> Result<(), SurfaceError> {
        self.inner.go_back()
    }

    fn go_forward(&mut self) -> Result<(), SurfaceError> {
        self.inner.go_forward()
    }

    fn can_go_back(&self) -> bool {
        self.inner.can_go_back()
    }

    fn can_go_forward(&self) -> bool {
        self.inner.can_go_forward()
    }

    fn set_cookie(&mut self, cookie: &Cookie) -> Result<(), SurfaceError> {
        self.inner.set_cookie(cookie)
    }

    fn get_cookies_for_url(&mut self, url: &str) -> Result<Vec<Cookie>, SurfaceError> {
        self.inner.get_cookies_for_url(url)
    }

    fn delete_cookie(&mut self, cookie: &Cookie) -> Result<(), SurfaceError> {
        self.inner.delete_cookie(cookie)
    }

    fn execute_script_with_result(&mut self, script: &str) -> Result<String, SurfaceError> {
        self.inner.execute_script_with_result(script)
    }

    fn poll_web_event(&mut self) -> Option<WebSurfaceEvent> {
        if let Some(event) = self.inner.poll_navigation_event().map(nav_to_web_event) {
            return Some(event);
        }
        self.inner
            .poll_web_message()
            .map(WebSurfaceEvent::WebMessage)
    }

    fn poll_navigation_event(&mut self) -> Option<NavigationEvent> {
        self.inner.poll_navigation_event()
    }

    fn poll_web_message(&mut self) -> Option<WebMessage> {
        self.inner.poll_web_message()
    }
}

fn nav_to_web_event(event: NavigationEvent) -> WebSurfaceEvent {
    match event {
        NavigationEvent::Started { .. }
        | NavigationEvent::Committed { .. }
        | NavigationEvent::Finished { .. }
        | NavigationEvent::Failed { .. } => WebSurfaceEvent::Navigation(event),
    }
}
