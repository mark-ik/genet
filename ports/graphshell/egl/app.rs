/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use base::generic_channel::GenericCallback;
use dpi::PhysicalSize;
use euclid::{Rect, Scale};
use keyboard_types::{CompositionEvent, CompositionState, Key, KeyState, NamedKey};
use log::{info, warn};
use raw_window_handle::{DisplayHandle, RawWindowHandle, WindowHandle};
use servo::{
    DeviceIndependentIntRect, DeviceIndependentPixel, DeviceIntSize, DevicePixel, DevicePoint,
    DeviceVector2D, EmbedderControl, EmbedderControlId, EventLoopWaker, ImeEvent, InputEvent,
    KeyboardEvent, LoadStatus, MediaSessionActionType, MediaSessionEvent, MouseButton,
    MouseButtonAction, MouseButtonEvent, MouseMoveEvent, Opts, Preferences, RefreshDriver,
    RenderingContext, ScreenGeometry, Scroll, Servo, ServoBuilder, SimpleDialog, TouchEvent,
    TouchEventType, TouchId, UserContentManager, WebView, WebViewId, WindowRenderingContext,
    convert_rect_to_css_pixel,
};
use url::Url;

use crate::egl::host_trait::HostTrait;
use crate::parser::location_bar_input_to_url;
use crate::prefs::ServoShellPreferences;
use crate::running_app_state::RunningAppState;
use crate::window::{PlatformWindow, ServoShellWindow, ServoShellWindowId};

const INPUT_TARGET_FALLBACK_WARN_INTERVAL: Duration = Duration::from_secs(60);

fn resolve_input_target_webview_id(
    preferred: Option<WebViewId>,
    newest: Option<WebViewId>,
) -> (Option<WebViewId>, bool) {
    match preferred {
        Some(webview_id) => (Some(webview_id), false),
        None => (newest, newest.is_some()),
    }
}

pub(crate) struct EmbeddedPlatformWindow {
    host: Rc<dyn HostTrait>,
    rendering_context: Rc<WindowRenderingContext>,
    refresh_driver: Rc<VsyncRefreshDriver>,
    viewport_rect: RefCell<Rect<i32, DevicePixel>>,
    /// The HiDPI scaling factor to use for the display of [`WebView`]s.
    hidpi_scale_factor: Scale<f32, DeviceIndependentPixel, DevicePixel>,
    /// A list of showing [`InputMethod`] interfaces.
    visible_input_methods: RefCell<Vec<EmbedderControlId>>,
    /// The current title of the active WebView in this window.
    current_title: RefCell<Option<String>>,
    /// The current URL of the active WebView in this window.
    current_url: RefCell<Option<Url>>,
    /// Whether or not the active WebView is currently able to go back.
    current_can_go_back: Cell<bool>,
    /// Whether or not the active WebView is currently able to go forward.
    current_can_go_forward: Cell<bool>,
    /// The current load status of the active WebView.
    current_load_status: Cell<Option<LoadStatus>>,
}

impl PlatformWindow for EmbeddedPlatformWindow {
    fn as_headed_window(&self) -> Option<&Self> {
        Some(self)
    }

    fn id(&self) -> ServoShellWindowId {
        0.into()
    }

    fn screen_geometry(&self) -> ScreenGeometry {
        let viewport_rect = self.viewport_rect.borrow();
        ScreenGeometry {
            size: viewport_rect.size,
            available_size: viewport_rect.size,
            window_rect: viewport_rect.to_box2d(),
        }
    }

    fn device_hidpi_scale_factor(&self) -> Scale<f32, DeviceIndependentPixel, DevicePixel> {
        self.hidpi_scale_factor
    }

    fn hidpi_scale_factor(&self) -> Scale<f32, DeviceIndependentPixel, DevicePixel> {
        self.hidpi_scale_factor
    }

    fn get_fullscreen(&self) -> bool {
        false
    }

    fn rebuild_user_interface(&self, _: &RunningAppState, _: &ServoShellWindow) {}

    #[cfg_attr(target_os = "android", expect(unused_variables))]
    fn update_user_interface_state(
        &self,
        state: &RunningAppState,
        window: &ServoShellWindow,
    ) -> bool {
        let Some(webview_id) = self.preferred_input_webview_id(window) else {
            return false;
        };
        let Some(active_webview) = window.webview_by_id(webview_id) else {
            return false;
        };

        let new_title = active_webview.page_title();
        let title_changed = new_title != *self.current_title.borrow();
        if title_changed {
            *self.current_title.borrow_mut() = new_title.clone();
            self.host.on_title_changed(new_title);
        }

        let new_url = active_webview.url();
        let url_changed = new_url != *self.current_url.borrow();
        if url_changed {
            let new_url_string = new_url.as_ref().map(Url::to_string).unwrap_or_default();
            *self.current_url.borrow_mut() = new_url;
            self.host.on_url_changed(new_url_string);
        }

        let new_back_forward = (
            active_webview.can_go_back(),
            active_webview.can_go_forward(),
        );
        let old_back_forward = (
            self.current_can_go_back.get(),
            self.current_can_go_forward.get(),
        );
        let back_forward_changed = new_back_forward != old_back_forward;
        if back_forward_changed {
            self.current_can_go_back.set(new_back_forward.0);
            self.current_can_go_forward.set(new_back_forward.1);
            self.host
                .on_history_changed(new_back_forward.0, new_back_forward.1);
        }

        let new_load_status = active_webview.load_status();
        let load_status_changed = Some(new_load_status) != self.current_load_status.get();
        if load_status_changed {
            self.host.notify_load_status_changed(new_load_status);

            #[cfg(all(feature = "tracing", feature = "tracing-hitrace"))]
            if new_load_status == LoadStatus::Complete {
                let (callback, receiver) =
                    GenericCallback::new_blocking().expect("Could not create channel");
                state.servo().create_memory_report(callback);
                std::thread::spawn(move || {
                    let result = receiver.recv().expect("Could not get memory report");
                    let reports = result
                        .results
                        .first()
                        .expect("We should have some memory report");
                    for report in &reports.reports {
                        let path = String::from("servo_memory_profiling:") + &report.path.join("/");
                        hitrace::trace_metric_str(&path, report.size as i64);
                    }
                });
            }
        }

        title_changed || url_changed || back_forward_changed || load_status_changed
    }

    fn request_repaint(&self, window: &ServoShellWindow) {
        window.repaint_webviews();
    }

    fn request_resize(&self, _: &WebView, _: DeviceIntSize) -> Option<DeviceIntSize> {
        None
    }

    fn rendering_context(&self) -> Rc<dyn RenderingContext> {
        self.rendering_context.clone()
    }

    fn window_rect(&self) -> DeviceIndependentIntRect {
        convert_rect_to_css_pixel(
            self.viewport_rect.borrow().to_box2d(),
            self.hidpi_scale_factor(),
        )
    }

    fn show_embedder_control(&self, _: WebViewId, embedder_control: EmbedderControl) {
        let control_id = embedder_control.id();
        match embedder_control {
            EmbedderControl::InputMethod(input_method_control) => {
                self.visible_input_methods.borrow_mut().push(control_id);
                self.host.on_ime_show(input_method_control);
            },
            EmbedderControl::SimpleDialog(simple_dialog) => match simple_dialog {
                SimpleDialog::Alert(alert_dialog) => {
                    self.host.show_alert(alert_dialog.message().into());
                    alert_dialog.confirm();
                },
                _ => {}, // The drop implementation will send the default response.
            },
            _ => {},
        }
    }

    fn hide_embedder_control(&self, _: WebViewId, control_id: servo::EmbedderControlId) {
        let mut visible_input_methods = self.visible_input_methods.borrow_mut();
        if let Some(index) = visible_input_methods
            .iter()
            .position(|visible_id| *visible_id == control_id)
        {
            visible_input_methods.remove(index);
            self.host.on_ime_hide();
        }
    }

    fn notify_media_session_event(&self, event: MediaSessionEvent) {
        match event {
            MediaSessionEvent::SetMetadata(metadata) => {
                self.host
                    .on_media_session_metadata(metadata.title, metadata.artist, metadata.album)
            },
            MediaSessionEvent::PlaybackStateChange(state) => {
                self.host.on_media_session_playback_state_change(state)
            },
            MediaSessionEvent::SetPositionState(position_state) => {
                self.host.on_media_session_set_position_state(
                    position_state.duration,
                    position_state.position,
                    position_state.playback_rate,
                )
            },
        };
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, backtrace: Option<String>) {
        self.host.on_panic(reason, backtrace);
    }

    fn show_console_message(&self, level: servo::ConsoleLogLevel, message: &str) {
        log::log!(level.into(), "{message}");
    }
}

#[derive(Default)]
pub(crate) struct VsyncRefreshDriver {
    start_frame_callbacks: RefCell<Vec<Box<dyn Fn() + Send>>>,
}

impl VsyncRefreshDriver {
    fn notify_vsync(&self) {
        let start_frame_callbacks: Vec<_> =
            self.start_frame_callbacks.borrow_mut().drain(..).collect();
        for start_frame_callback in start_frame_callbacks {
            start_frame_callback()
        }
    }
}

impl RefreshDriver for VsyncRefreshDriver {
    fn observe_next_frame(&self, new_start_frame_callback: Box<dyn Fn() + Send + 'static>) {
        self.start_frame_callbacks
            .borrow_mut()
            .push(new_start_frame_callback);
    }
}

pub(crate) struct AppInitOptions {
    pub host: Rc<dyn HostTrait>,
    pub event_loop_waker: Box<dyn EventLoopWaker>,
    pub initial_url: Option<String>,
    pub opts: Opts,
    pub preferences: Preferences,
    pub servoshell_preferences: ServoShellPreferences,
    #[cfg(feature = "webxr")]
    pub xr_discovery: Option<servo::webxr::Discovery>,
}

pub struct App {
    state: Rc<RunningAppState>,
    // Known limitation: EGL embedder currently operates as a single-window app.
    // Desktop supports multiple windows; EGL can be extended similarly later.
    host: Rc<dyn HostTrait>,
    initial_url: Url,
    input_target_fallback_last_warned_at: Cell<Option<Instant>>,
    input_target_fallback_suppressed: Cell<u64>,
}

#[expect(unused)]
impl App {
    pub(super) fn new(init: AppInitOptions) -> Rc<Self> {
        let mut servo_builder = ServoBuilder::default()
            .opts(init.opts)
            .preferences(init.preferences.clone())
            .event_loop_waker(init.event_loop_waker.clone());
        #[cfg(feature = "webxr")]
        let servo_builder = servo_builder
            .webxr_registry(Box::new(XrDiscoveryWebXrRegistry::new(init.xr_discovery)));
        let servo = servo_builder.build();

        let initial_url = init.initial_url.and_then(|string| Url::parse(&string).ok());
        let initial_url = initial_url
            .or_else(|| Url::parse(&init.servoshell_preferences.homepage).ok())
            .or_else(|| Url::parse("about:blank").ok())
            .expect("Failed to parse initial URL");

        let user_content_manager = Rc::new(UserContentManager::new(&servo));
        let state = Rc::new(RunningAppState::new(
            servo,
            init.servoshell_preferences,
            init.event_loop_waker,
            user_content_manager,
            init.preferences,
        ));

        Rc::new(Self {
            state,
            host: init.host,
            initial_url,
            input_target_fallback_last_warned_at: Cell::new(None),
            input_target_fallback_suppressed: Cell::new(0),
        })
    }

    pub(crate) fn add_platform_window(
        &self,
        display_handle: DisplayHandle,
        window_handle: WindowHandle,
        viewport_rect: Rect<i32, DevicePixel>,
        hidpi_scale_factor: Scale<f32, DeviceIndependentPixel, DevicePixel>,
    ) {
        let viewport_size = viewport_rect.size;
        let refresh_driver = Rc::new(VsyncRefreshDriver::default());
        let rendering_context = Rc::new(
            WindowRenderingContext::new_with_refresh_driver(
                display_handle,
                window_handle,
                PhysicalSize::new(viewport_size.width as u32, viewport_size.height as u32),
                refresh_driver.clone(),
            )
            .expect("Could not create RenderingContext"),
        );
        let platform_window = Rc::new(EmbeddedPlatformWindow {
            host: self.host.clone(),
            rendering_context,
            refresh_driver,
            viewport_rect: RefCell::new(viewport_rect),
            hidpi_scale_factor,
            visible_input_methods: Default::default(),
            current_title: Default::default(),
            current_url: Default::default(),
            current_can_go_back: Default::default(),
            current_can_go_forward: Default::default(),
            current_load_status: Default::default(),
        });
        self.state
            .open_window(platform_window.clone(), self.initial_url.clone());
    }

    pub(crate) fn servo(&self) -> &Servo {
        &self.state.servo
    }

    pub(crate) fn servoshell_preferences(&self) -> &ServoShellPreferences {
        &self.state.servoshell_preferences
    }

    pub(crate) fn window(&self) -> Rc<ServoShellWindow> {
        self.state
            .windows()
            .values()
            .nth(0)
            .expect("Should always have one open window")
            .clone()
    }

    fn warn_input_target_fallback_once_per_interval(&self) {
        let now = Instant::now();
        let should_warn = match self.input_target_fallback_last_warned_at.get() {
            None => true,
            Some(last) => now.duration_since(last) >= INPUT_TARGET_FALLBACK_WARN_INTERVAL,
        };
        if should_warn {
            let suppressed = self.input_target_fallback_suppressed.replace(0);
            warn!(
                "input_target_webview: preferred_input returned None, falling back to newest() (suppressed_repeats={suppressed})"
            );
            self.input_target_fallback_last_warned_at.set(Some(now));
        } else {
            self.input_target_fallback_suppressed.set(
                self.input_target_fallback_suppressed
                    .get()
                    .saturating_add(1),
            );
        }
    }

    fn input_target_webview_id(&self) -> Option<WebViewId> {
        let window = self.window();
        let preferred = window.platform_window().preferred_input_webview_id(&window);
        let newest = window
            .webview_collection
            .borrow()
            .newest()
            .map(|wv| wv.id());
        let (resolved, used_fallback) = resolve_input_target_webview_id(preferred, newest);
        if used_fallback {
            self.warn_input_target_fallback_once_per_interval();
        }
        resolved
    }

    fn input_target_webview(&self) -> Option<WebView> {
        let webview_id = self.input_target_webview_id()?;
        self.window().webview_by_id(webview_id)
    }

    pub(crate) fn initial_url(&self) -> Url {
        self.initial_url.clone()
    }

    pub(crate) fn create_and_activate_toplevel_webview(self: &Rc<Self>, url: Url) -> WebView {
        self.window()
            .create_and_activate_toplevel_webview(self.state.clone(), url)
    }

    /// The active webview will be immediately valid via `input_target_webview()`
    pub(crate) fn activate_webview(&self, id: WebViewId) {
        self.state.window_for_webview_id(id).activate_webview(id);
    }

    /// This is the Servo heartbeat. This needs to be called
    /// everytime wakeup is called or when embedder wants Servo
    /// to act on its pending events.
    pub fn spin_event_loop(&self) {
        if !self
            .state
            .spin_event_loop(None /* create_platform_window */)
        {
            self.host.on_shutdown_complete()
        }
    }

    /// Load an URL.
    pub fn load_uri(&self, location: &str) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.load_uri_for_webview(webview_id, location);
    }

    // NOTE: `_for_webview` entrypoints are intentionally public host-facing APIs.
    // Wrapper methods above preserve compatibility for callers that do not pass IDs.
    /// Load an URL in a specific WebView.
    pub fn load_uri_for_webview(&self, webview_id: WebViewId, location: &str) {
        let Some(url) =
            location_bar_input_to_url(location, &self.servoshell_preferences().searchpage)
        else {
            warn!("failed to parse location");
            return;
        };
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            self.window().set_needs_update();
            webview.load(url.into_url());
        }
        self.spin_event_loop();
    }

    /// Reload the page.
    pub fn reload(&self) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.reload_for_webview(webview_id);
    }

    /// Reload a specific WebView.
    pub fn reload_for_webview(&self, webview_id: WebViewId) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            self.window().set_needs_update();
            webview.reload();
            self.spin_event_loop();
        }
    }

    /// Stop loading the page.
    pub fn stop(&self) {
        // Hard gap: Servo's public WebView API does not expose stop_loading().
        // The DOM has Window::stop_loading() (components/script/dom/window.rs)
        // but there is no public embedder API equivalent on WebView today.
        // Keep this as a no-op entrypoint for platform callers.
        self.spin_event_loop();
    }

    /// Go back in history.
    pub fn go_back(&self) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.go_back_for_webview(webview_id);
    }

    /// Go back in history for a specific WebView.
    pub fn go_back_for_webview(&self, webview_id: WebViewId) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.go_back(1);
            self.spin_event_loop();
        }
    }

    /// Go forward in history.
    pub fn go_forward(&self) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.go_forward_for_webview(webview_id);
    }

    /// Go forward in history for a specific WebView.
    pub fn go_forward_for_webview(&self, webview_id: WebViewId) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.go_forward(1);
            self.spin_event_loop();
        }
    }

    /// Let Servo know that the window has been resized.
    pub fn resize(&self, viewport_rect: Rect<i32, DevicePixel>) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.resize_for_webview(webview_id, viewport_rect);
    }

    /// Let Servo know that the window has been resized for a specific WebView.
    pub fn resize_for_webview(&self, webview_id: WebViewId, viewport_rect: Rect<i32, DevicePixel>) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            info!("Setting viewport to {viewport_rect:?}");
            let size = viewport_rect.size;
            webview.resize(PhysicalSize::new(size.width as u32, size.height as u32));
        }
        let window = self.window().platform_window();
        let embedded_platform_window = window.as_headed_window().expect("No headed window");
        *embedded_platform_window.viewport_rect.borrow_mut() = viewport_rect;

        self.spin_event_loop();
    }

    /// Scroll.
    /// x/y are scroll coordinates.
    /// dx/dy are scroll deltas.
    pub fn scroll(&self, dx: f32, dy: f32, x: f32, y: f32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.scroll_for_webview(webview_id, dx, dy, x, y);
    }

    /// Scroll a specific WebView.
    pub fn scroll_for_webview(&self, webview_id: WebViewId, dx: f32, dy: f32, x: f32, y: f32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            let scroll = Scroll::Delta(DeviceVector2D::new(dx, dy).into());
            let point = DevicePoint::new(x, y).into();
            webview.notify_scroll_event(scroll, point);
            self.spin_event_loop();
        }
    }

    /// Touch event: press down
    pub fn touch_down(&self, x: f32, y: f32, pointer_id: i32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.touch_down_for_webview(webview_id, x, y, pointer_id);
    }

    /// Touch event: press down for a specific WebView.
    pub fn touch_down_for_webview(&self, webview_id: WebViewId, x: f32, y: f32, pointer_id: i32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Touch(TouchEvent::new(
                TouchEventType::Down,
                TouchId(pointer_id),
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Touch event: move touching finger
    pub fn touch_move(&self, x: f32, y: f32, pointer_id: i32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.touch_move_for_webview(webview_id, x, y, pointer_id);
    }

    /// Touch event: move touching finger for a specific WebView.
    pub fn touch_move_for_webview(&self, webview_id: WebViewId, x: f32, y: f32, pointer_id: i32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Touch(TouchEvent::new(
                TouchEventType::Move,
                TouchId(pointer_id),
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Touch event: Lift touching finger
    pub fn touch_up(&self, x: f32, y: f32, pointer_id: i32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.touch_up_for_webview(webview_id, x, y, pointer_id);
    }

    /// Touch event: Lift touching finger for a specific WebView.
    pub fn touch_up_for_webview(&self, webview_id: WebViewId, x: f32, y: f32, pointer_id: i32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Touch(TouchEvent::new(
                TouchEventType::Up,
                TouchId(pointer_id),
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Cancel touch event
    pub fn touch_cancel(&self, x: f32, y: f32, pointer_id: i32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.touch_cancel_for_webview(webview_id, x, y, pointer_id);
    }

    /// Cancel touch event for a specific WebView.
    pub fn touch_cancel_for_webview(&self, webview_id: WebViewId, x: f32, y: f32, pointer_id: i32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Touch(TouchEvent::new(
                TouchEventType::Cancel,
                TouchId(pointer_id),
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Register a mouse movement.
    pub fn mouse_move(&self, x: f32, y: f32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.mouse_move_for_webview(webview_id, x, y);
    }

    /// Register a mouse movement for a specific WebView.
    pub fn mouse_move_for_webview(&self, webview_id: WebViewId, x: f32, y: f32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Register a mouse button press.
    pub fn mouse_down(&self, x: f32, y: f32, button: MouseButton) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.mouse_down_for_webview(webview_id, x, y, button);
    }

    /// Register a mouse button press for a specific WebView.
    pub fn mouse_down_for_webview(
        &self,
        webview_id: WebViewId,
        x: f32,
        y: f32,
        button: MouseButton,
    ) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Down,
                button,
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Register a mouse button release.
    pub fn mouse_up(&self, x: f32, y: f32, button: MouseButton) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.mouse_up_for_webview(webview_id, x, y, button);
    }

    /// Register a mouse button release for a specific WebView.
    pub fn mouse_up_for_webview(&self, webview_id: WebViewId, x: f32, y: f32, button: MouseButton) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Up,
                button,
                DevicePoint::new(x, y).into(),
            )));
            self.spin_event_loop();
        }
    }

    /// Start pinchzoom.
    /// x/y are pinch origin coordinates.
    pub fn pinchzoom_start(&self, factor: f32, x: f32, y: f32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.pinchzoom_start_for_webview(webview_id, factor, x, y);
    }

    /// Start pinchzoom for a specific WebView.
    pub fn pinchzoom_start_for_webview(&self, webview_id: WebViewId, factor: f32, x: f32, y: f32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.pinch_zoom(factor, DevicePoint::new(x, y));
            self.spin_event_loop();
        }
    }

    /// Pinchzoom.
    /// x/y are pinch origin coordinates.
    pub fn pinchzoom(&self, factor: f32, x: f32, y: f32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.pinchzoom_for_webview(webview_id, factor, x, y);
    }

    /// Pinchzoom for a specific WebView.
    pub fn pinchzoom_for_webview(&self, webview_id: WebViewId, factor: f32, x: f32, y: f32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.pinch_zoom(factor, DevicePoint::new(x, y));
            self.spin_event_loop();
        }
    }

    /// End pinchzoom.
    /// x/y are pinch origin coordinates.
    pub fn pinchzoom_end(&self, factor: f32, x: f32, y: f32) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.pinchzoom_end_for_webview(webview_id, factor, x, y);
    }

    /// End pinchzoom for a specific WebView.
    pub fn pinchzoom_end_for_webview(&self, webview_id: WebViewId, factor: f32, x: f32, y: f32) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.pinch_zoom(factor, DevicePoint::new(x, y));
            self.spin_event_loop();
        }
    }

    pub fn key_down(&self, key: Key) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.key_down_for_webview(webview_id, key);
    }

    pub fn key_down_for_webview(&self, webview_id: WebViewId, key: Key) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                KeyState::Down,
                key,
            )));
            self.spin_event_loop();
        }
    }

    pub fn key_up(&self, key: Key) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.key_up_for_webview(webview_id, key);
    }

    pub fn key_up_for_webview(&self, webview_id: WebViewId, key: Key) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                KeyState::Up,
                key,
            )));
            self.spin_event_loop();
        }
    }

    pub fn ime_insert_text(&self, text: String) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.ime_insert_text_for_webview(webview_id, text);
    }

    pub fn ime_insert_text_for_webview(&self, webview_id: WebViewId, text: String) {
        // In OHOS, we get empty text after the intended text.
        if text.is_empty() {
            return;
        }

        let Some(webview) = self.window().webview_by_id(webview_id) else {
            return;
        };

        webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
            KeyState::Down,
            Key::Named(NamedKey::Process),
        )));
        webview.notify_input_event(InputEvent::Ime(ImeEvent::Composition(CompositionEvent {
            state: CompositionState::End,
            data: text,
        })));
        webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
            KeyState::Up,
            Key::Named(NamedKey::Process),
        )));
        self.spin_event_loop();
    }

    pub fn media_session_action(&self, action: MediaSessionActionType) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.media_session_action_for_webview(webview_id, action);
    }

    pub fn media_session_action_for_webview(
        &self,
        webview_id: WebViewId,
        action: MediaSessionActionType,
    ) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_media_session_action_event(action);
            self.spin_event_loop();
        }
    }

    pub fn set_throttled(&self, throttled: bool) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.set_throttled_for_webview(webview_id, throttled);
    }

    pub fn set_throttled_for_webview(&self, webview_id: WebViewId, throttled: bool) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.set_throttled(throttled);
            self.spin_event_loop();
        }
    }

    pub fn ime_dismissed(&self) {
        let Some(webview_id) = self.input_target_webview_id() else {
            return;
        };
        self.ime_dismissed_for_webview(webview_id);
    }

    pub fn ime_dismissed_for_webview(&self, webview_id: WebViewId) {
        if let Some(webview) = self.window().webview_by_id(webview_id) {
            webview.notify_input_event(InputEvent::Ime(ImeEvent::Dismissed));
            self.spin_event_loop();
        }
    }

    // Current design keeps vsync ownership in the platform embedder, which forwards
    // ticks into the refresh driver.
    pub fn notify_vsync(&self) {
        let platform_window = self.window().platform_window();
        let embedded_platform_window = platform_window
            .as_headed_window()
            .expect("No headed window");
        embedded_platform_window.refresh_driver.notify_vsync();
        self.spin_event_loop();
    }

    pub fn pause_painting(&self) {
        let platform_window = self.window().platform_window();
        let embedded_platform_window = platform_window
            .as_headed_window()
            .expect("No headed window");
        if let Err(error) = embedded_platform_window.rendering_context.take_window() {
            warn!("Unbinding native surface from context failed ({error:?})");
        }
        self.spin_event_loop();
    }

    pub fn resume_painting(
        &self,
        window_handle: RawWindowHandle,
        viewport_rect: Rect<i32, DevicePixel>,
    ) {
        let window_handle = unsafe { WindowHandle::borrow_raw(window_handle) };
        let size = viewport_rect.size.to_u32();
        let platform_window = self.window().platform_window();
        let embedded_platform_window = platform_window
            .as_headed_window()
            .expect("No headed window");
        if let Err(error) = embedded_platform_window
            .rendering_context
            .set_window(window_handle, PhysicalSize::new(size.width, size.height))
        {
            warn!("Binding native surface to context failed ({error:?})");
        }
        self.spin_event_loop();
    }
}

#[cfg(feature = "webxr")]
pub(crate) struct XrDiscoveryWebXrRegistry {
    xr_discovery: RefCell<Option<servo::webxr::Discovery>>,
}

#[cfg(feature = "webxr")]
impl XrDiscoveryWebXrRegistry {
    pub(crate) fn new(xr_discovery: Option<servo::webxr::Discovery>) -> Self {
        Self {
            xr_discovery: RefCell::new(xr_discovery),
        }
    }
}

#[cfg(feature = "webxr")]
impl servo::webxr::WebXrRegistry for XrDiscoveryWebXrRegistry {
    fn register(&self, registry: &mut servo::webxr::MainThreadRegistry) {
        log::debug!("XrDiscoveryWebXrRegistry::register");
        if let Some(discovery) = self.xr_discovery.take() {
            registry.register(discovery);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_input_target_webview_id;
    use base::id::PainterId;
    use servo::WebViewId;

    fn test_webview_id() -> WebViewId {
        WebViewId::new(PainterId::next())
    }

    #[test]
    fn test_resolve_input_target_webview_id_prefers_preferred() {
        let preferred = test_webview_id();
        let newest = test_webview_id();
        let (resolved, used_fallback) =
            resolve_input_target_webview_id(Some(preferred), Some(newest));
        assert_eq!(resolved, Some(preferred));
        assert!(!used_fallback);
    }

    #[test]
    fn test_resolve_input_target_webview_id_falls_back_to_newest() {
        let newest = test_webview_id();
        let (resolved, used_fallback) = resolve_input_target_webview_id(None, Some(newest));
        assert_eq!(resolved, Some(newest));
        assert!(used_fallback);
    }

    #[test]
    fn test_resolve_input_target_webview_id_none_when_no_targets() {
        let (resolved, used_fallback) = resolve_input_target_webview_id(None, None);
        assert_eq!(resolved, None);
        assert!(!used_fallback);
    }
}
