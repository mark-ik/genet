/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ScryingProducer` — adapter wrapping a `Box<dyn scrying::WebSurfaceProducer>`
//! to satisfy `inker::SurfaceProducer`. Delegates each method via the
//! translation helpers in [`crate::translation`].
//!
//! Web navigation is deliberately non-blocking here: `navigate_to_url` and
//! `navigate_to_string` begin a load and return once the platform producer has
//! accepted the request.

use std::time::Duration;

use inker::{
    Cookie, CursorShape, FocusReason, KeyboardEvent, MouseEvent, NavigationEvent, PointerEvent,
    SurfaceError, SurfaceFrame, SurfaceProducer, SurfaceSettings, WebMessage, WebSurface,
    WebSurfaceCapabilities, WebSurfaceEvent,
};
use scrying::WebSurfaceProducer;

use crate::translation::{
    map_capabilities, map_cookie, map_cursor_shape, map_error, map_focus_reason, map_frame,
    map_keyboard, map_mouse, map_navigation_event, map_pointer, map_settings, map_web_event,
    wrap_web_message,
};

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(3);
const INITIAL_EMPTY_POLL_STALL_THRESHOLD: u32 = 600;
const MAX_EMPTY_POLL_STALL_THRESHOLD: u32 = 4_800;

pub struct ScryingProducer {
    inner: Box<dyn WebSurfaceProducer>,
    fence_handle: Option<u64>,
    empty_polls: u32,
    stall_threshold: u32,
}

impl ScryingProducer {
    pub fn new(inner: Box<dyn WebSurfaceProducer>, fence_handle: Option<u64>) -> Self {
        Self {
            inner,
            fence_handle,
            empty_polls: 0,
            stall_threshold: INITIAL_EMPTY_POLL_STALL_THRESHOLD,
        }
    }
}

impl SurfaceProducer for ScryingProducer {
    fn resize(&mut self, width: u32, height: u32) -> Result<(), SurfaceError> {
        self.inner
            .resize(dpi::PhysicalSize::new(width, height))
            .map_err(map_error)
    }

    fn set_offset(&mut self, x: i32, y: i32) -> Result<(), SurfaceError> {
        self.inner.set_offset(x as f32, y as f32).map_err(map_error)
    }

    fn acquire_frame(&mut self) -> Result<Option<SurfaceFrame>, SurfaceError> {
        match self.inner.try_acquire_frame().map_err(map_error)? {
            Some(frame) => {
                self.empty_polls = 0;
                self.stall_threshold = INITIAL_EMPTY_POLL_STALL_THRESHOLD;
                Ok(map_frame(frame, self.fence_handle))
            }
            None => {
                self.empty_polls = self.empty_polls.saturating_add(1);
                if self.empty_polls >= self.stall_threshold {
                    self.inner
                        .restart_capture_after_stall()
                        .map_err(map_error)?;
                    self.empty_polls = 0;
                    self.stall_threshold = self
                        .stall_threshold
                        .saturating_mul(2)
                        .min(MAX_EMPTY_POLL_STALL_THRESHOLD);
                }
                Ok(None)
            }
        }
    }

    fn send_mouse_input(&mut self, ev: MouseEvent) -> Result<(), SurfaceError> {
        self.inner
            .send_mouse_input(map_mouse(ev))
            .map_err(map_error)
    }

    fn send_pointer_input(&mut self, ev: PointerEvent) -> Result<(), SurfaceError> {
        self.inner
            .send_pointer_input(map_pointer(ev))
            .map_err(map_error)
    }

    fn send_keyboard_input(&mut self, ev: KeyboardEvent) -> Result<(), SurfaceError> {
        self.inner
            .send_keyboard_input(map_keyboard(ev))
            .map_err(map_error)
    }

    fn move_focus(&mut self, reason: FocusReason) -> Result<(), SurfaceError> {
        self.inner
            .move_focus(map_focus_reason(reason))
            .map_err(map_error)
    }

    fn poll_cursor_shape(&mut self) -> Option<CursorShape> {
        self.inner.poll_cursor_shape().map(map_cursor_shape)
    }

    fn apply_settings(&mut self, settings: &SurfaceSettings) -> Result<(), SurfaceError> {
        self.inner
            .apply_settings(&map_settings(settings))
            .map_err(map_error)
    }

    fn capture_snapshot_png(&mut self) -> Result<Vec<u8>, SurfaceError> {
        self.inner.capture_snapshot_png().map_err(map_error)
    }

    fn as_web_surface(&mut self) -> Option<&mut dyn WebSurface> {
        Some(self)
    }
}

impl WebSurface for ScryingProducer {
    fn capabilities(&self) -> WebSurfaceCapabilities {
        map_capabilities(self.inner.capabilities())
    }

    fn navigate_to_url(&mut self, url: &str) -> Result<(), SurfaceError> {
        self.inner.load_url(url).map_err(map_error)
    }

    fn navigate_to_string(&mut self, html: &str) -> Result<(), SurfaceError> {
        self.inner.load_html(html).map_err(map_error)
    }

    fn reload(&mut self) -> Result<(), SurfaceError> {
        self.inner.reload().map_err(map_error)
    }

    fn stop(&mut self) -> Result<(), SurfaceError> {
        self.inner.stop().map_err(map_error)
    }

    fn go_back(&mut self) -> Result<(), SurfaceError> {
        self.inner.go_back().map(|_| ()).map_err(map_error)
    }

    fn go_forward(&mut self) -> Result<(), SurfaceError> {
        self.inner.go_forward().map(|_| ()).map_err(map_error)
    }

    fn can_go_back(&self) -> bool {
        self.inner.can_go_back()
    }

    fn can_go_forward(&self) -> bool {
        self.inner.can_go_forward()
    }

    fn set_cookie(&mut self, cookie: &Cookie) -> Result<(), SurfaceError> {
        self.inner
            .set_cookie(&map_cookie(cookie))
            .map_err(map_error)
    }

    fn execute_script_with_result(&mut self, script: &str) -> Result<String, SurfaceError> {
        self.inner
            .execute_script_with_result(script, SCRIPT_TIMEOUT)
            .map_err(map_error)
    }

    fn poll_web_event(&mut self) -> Option<WebSurfaceEvent> {
        if let Some(event) = self.inner.poll_navigation_event().and_then(map_web_event) {
            return Some(event);
        }
        self.inner
            .poll_web_message()
            .map(wrap_web_message)
            .map(WebSurfaceEvent::WebMessage)
    }

    fn poll_navigation_event(&mut self) -> Option<NavigationEvent> {
        self.inner
            .poll_navigation_event()
            .and_then(map_navigation_event)
    }

    fn poll_web_message(&mut self) -> Option<WebMessage> {
        self.inner.poll_web_message().map(wrap_web_message)
    }
}
