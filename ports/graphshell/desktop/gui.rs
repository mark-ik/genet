/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Key, LayerId, Modifiers, PaintCallback, TopBottomPanel, Vec2, WidgetInfo, WidgetType, pos2,
};
use egui_glow::{CallbackFn, EguiGlow};
use egui_tiles::{Container, Tile, Tiles, Tree};
use egui_winit::EventResponse;
use euclid::{Length, Point2D, Rect, Scale, Size2D};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use log::warn;
use servo::{
    DeviceIndependentPixel, DevicePixel, Image, LoadStatus, OffscreenRenderingContext, PixelFormat,
    RenderingContext, WebViewId, WindowRenderingContext,
};
use url::Url;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use super::tile_behavior::GraphshellTileBehavior;
use super::tile_kind::TileKind;
use super::webview_controller;
use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::event_loop::AppEvent;
use crate::desktop::headed_window;
use crate::graph::{NodeKey, NodeLifecycle};
use crate::input;
use crate::render;
use crate::running_app_state::{RunningAppState, UserInterfaceCommand};
use crate::search::fuzzy_match_node_keys;
use crate::window::{GraphSemanticEvent, ServoShellWindow};

/// The user interface of a headed servoshell. Currently this is implemented via
/// egui.
pub struct Gui {
    rendering_context: Rc<OffscreenRenderingContext>,
    window_rendering_context: Rc<WindowRenderingContext>,
    context: EguiGlow,
    /// Tile tree backing graph/detail pane layout.
    tiles_tree: Tree<TileKind>,
    toolbar_height: Length<f32, DeviceIndependentPixel>,

    location: String,

    /// Whether the location has been edited by the user without clicking Go.
    location_dirty: bool,

    /// Whether the address bar Enter was pressed (consumed on next frame).
    location_submitted: bool,

    /// Whether to show the "clear saved graph data" confirmation dialog.
    show_clear_data_confirm: bool,

    /// Whether to show runtime persistence directory switch dialog.
    show_data_dir_dialog: bool,

    /// Current editable persistence directory path.
    data_dir_input: String,

    /// Last status message for persistence directory switching.
    data_dir_status: Option<String>,

    /// Whether to show persistence settings dialog.
    show_persistence_settings_dialog: bool,

    /// Snapshot interval input value (seconds).
    snapshot_interval_input: String,

    /// Last status message for persistence settings updates.
    persistence_settings_status: Option<String>,

    /// The [`LoadStatus`] of the active `WebView`.
    load_status: LoadStatus,

    /// The text to display in the status bar on the bottom of the window.
    status_text: Option<String>,

    /// Whether or not the current `WebView` can navigate backward.
    can_go_back: bool,

    /// Whether or not the current `WebView` can navigate forward.
    can_go_forward: bool,

    /// Handle to the GPU texture of the favicon.
    ///
    /// These need to be cached across egui draw calls.
    favicon_textures: HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,

    /// Graph browser application state
    graph_app: GraphBrowserApp,

    /// Per-node offscreen rendering contexts for WebView tiles.
    tile_rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>>,

    /// Per-node favicon textures for egui_tiles tab rendering.
    tile_favicon_textures: HashMap<NodeKey, (u64, egui::TextureHandle)>,

    /// Sender for asynchronous webview thumbnail capture results.
    thumbnail_capture_tx: Sender<ThumbnailCaptureResult>,

    /// Receiver for asynchronous webview thumbnail capture results.
    thumbnail_capture_rx: Receiver<ThumbnailCaptureResult>,

    /// WebViews with an in-flight thumbnail request.
    thumbnail_capture_in_flight: HashSet<WebViewId>,

    /// Whether the graph search UI is visible.
    graph_search_open: bool,

    /// Current graph search query text.
    graph_search_query: String,

    /// Search mode: hide non-matching nodes when enabled.
    graph_search_filter_mode: bool,

    /// Ranked node matches for the current search query.
    graph_search_matches: Vec<NodeKey>,

    /// Active result index in `graph_search_matches`.
    graph_search_active_match_index: Option<usize>,

    /// Runtime backpressure state for tile-driven webview creation retries.
    webview_creation_backpressure: HashMap<NodeKey, WebviewCreationBackpressureState>,

    /// Count of webview accessibility tree updates that could not be bridged.
    webview_accessibility_updates_dropped: u64,

    /// Whether we've already warned about dropped webview accessibility updates.
    webview_accessibility_warned: bool,

    /// Cached reference to RunningAppState for webview creation
    state: Option<Rc<RunningAppState>>,

    /// Last explicitly focused webview target in tile mode.
    focused_webview_hint: Option<WebViewId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenWebviewTileMode {
    Tab,
    SplitHorizontal,
}

// Pragmatic Phase A backpressure:
// Servo webview creation is not fallible in the embedder API, so we infer failure
// from "no semantic signal + no stable live webview" within a timeout window.
const WEBVIEW_CREATION_CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);
const WEBVIEW_CREATION_TIMEOUT: Duration = Duration::from_secs(8);
const WEBVIEW_CREATION_MAX_RETRIES: u8 = 3;

#[derive(Clone, Copy, Debug)]
struct WebviewCreationProbe {
    webview_id: WebViewId,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebviewCreationProbeOutcome {
    Confirmed,
    Pending,
    TimedOut,
}

#[derive(Default, Debug)]
struct WebviewCreationBackpressureState {
    retry_count: u8,
    pending: Option<WebviewCreationProbe>,
}

fn classify_webview_creation_probe(
    elapsed: Duration,
    contains_webview: bool,
    has_responsive_signal: bool,
) -> WebviewCreationProbeOutcome {
    if has_responsive_signal || (contains_webview && elapsed >= WEBVIEW_CREATION_CONFIRMATION_WINDOW)
    {
        WebviewCreationProbeOutcome::Confirmed
    } else if elapsed >= WEBVIEW_CREATION_TIMEOUT {
        WebviewCreationProbeOutcome::TimedOut
    } else {
        WebviewCreationProbeOutcome::Pending
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        if let Ok(layout_json) = serde_json::to_string(&self.tiles_tree) {
            self.graph_app.save_tile_layout_json(&layout_json);
        } else {
            warn!("Failed to serialize tile layout for persistence");
        }
        self.graph_app.take_snapshot();
        self.rendering_context
            .make_current()
            .expect("Could not make window RenderingContext current");
        self.context.destroy();
    }
}

impl Gui {
    pub(crate) fn new(
        winit_window: &Window,
        event_loop: &ActiveEventLoop,
        event_loop_proxy: EventLoopProxy<AppEvent>,
        rendering_context: Rc<OffscreenRenderingContext>,
        window_rendering_context: Rc<WindowRenderingContext>,
        initial_url: Url,
        graph_data_dir: Option<PathBuf>,
        graph_snapshot_interval_secs: Option<u64>,
    ) -> Self {
        rendering_context
            .make_current()
            .expect("Could not make window RenderingContext current");
        let mut context = EguiGlow::new(
            event_loop,
            rendering_context.glow_gl_api(),
            None,
            None,
            false,
        );

        context
            .egui_winit
            .init_accesskit(event_loop, winit_window, event_loop_proxy);
        winit_window.set_visible(true);

        context.egui_ctx.options_mut(|options| {
            // Disable the builtin egui handlers for the Ctrl+Plus, Ctrl+Minus and Ctrl+0
            // shortcuts as they don't work well with servoshell's `device-pixel-ratio` CLI argument.
            options.zoom_with_keyboard = false;

            // On platforms where winit fails to obtain a system theme, fall back to a light theme
            // since it is the more common default.
            options.fallback_theme = egui::Theme::Light;
        });

        let initial_data_dir =
            graph_data_dir.unwrap_or_else(crate::persistence::GraphStore::default_data_dir);
        let mut graph_app = GraphBrowserApp::new_from_dir(initial_data_dir.clone());
        if let Some(snapshot_secs) = graph_snapshot_interval_secs
            && let Err(e) = graph_app.set_snapshot_interval_secs(snapshot_secs)
        {
            warn!("Failed to apply snapshot interval from startup preferences: {e}");
        }
        let mut tiles = Tiles::default();
        let graph_tile_id = tiles.insert_pane(TileKind::Graph);
        let mut tiles_tree = Tree::new("graphshell_tiles", graph_tile_id, tiles);

        if let Some(layout_json) = graph_app.load_tile_layout_json()
            && let Ok(mut restored_tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json)
        {
            Self::prune_stale_webview_tile_keys_only(&mut restored_tree, &graph_app);
            if restored_tree.root().is_some() {
                tiles_tree = restored_tree;
            }
        }

        // Only create initial node if graph wasn't recovered from persistence
        if !graph_app.has_recovered_graph() {
            use euclid::default::Point2D;
            let _initial_node =
                graph_app.add_node_and_sync(initial_url.to_string(), Point2D::new(400.0, 300.0));
        }
        let (thumbnail_capture_tx, thumbnail_capture_rx) = channel();

        Self {
            rendering_context,
            window_rendering_context,
            context,
            tiles_tree,
            toolbar_height: Default::default(),
            location: initial_url.to_string(),
            location_dirty: false,
            location_submitted: false,
            show_clear_data_confirm: false,
            show_data_dir_dialog: false,
            data_dir_input: initial_data_dir.display().to_string(),
            data_dir_status: None,
            show_persistence_settings_dialog: false,
            snapshot_interval_input: graph_app
                .snapshot_interval_secs()
                .unwrap_or(crate::persistence::DEFAULT_SNAPSHOT_INTERVAL_SECS)
                .to_string(),
            persistence_settings_status: None,
            load_status: LoadStatus::Complete,
            status_text: None,
            can_go_back: false,
            can_go_forward: false,
            favicon_textures: Default::default(),
            graph_app,
            tile_rendering_contexts: HashMap::new(),
            tile_favicon_textures: HashMap::new(),
            thumbnail_capture_tx,
            thumbnail_capture_rx,
            thumbnail_capture_in_flight: HashSet::new(),
            graph_search_open: false,
            graph_search_query: String::new(),
            graph_search_filter_mode: false,
            graph_search_matches: Vec::new(),
            graph_search_active_match_index: None,
            webview_creation_backpressure: HashMap::new(),
            webview_accessibility_updates_dropped: 0,
            webview_accessibility_warned: false,
            state: None,
            focused_webview_hint: None,
        }
    }

    fn restore_tiles_tree_from_persistence(graph_app: &GraphBrowserApp) -> Tree<TileKind> {
        let mut tiles = Tiles::default();
        let graph_tile_id = tiles.insert_pane(TileKind::Graph);
        let mut tiles_tree = Tree::new("graphshell_tiles", graph_tile_id, tiles);
        if let Some(layout_json) = graph_app.load_tile_layout_json()
            && let Ok(mut restored_tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json)
        {
            Self::prune_stale_webview_tile_keys_only(&mut restored_tree, graph_app);
            if restored_tree.root().is_some() {
                tiles_tree = restored_tree;
            }
        }
        tiles_tree
    }

    fn switch_persistence_store(
        graph_app: &mut GraphBrowserApp,
        window: &ServoShellWindow,
        tiles_tree: &mut Tree<TileKind>,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        tile_favicon_textures: &mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
        favicon_textures: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
        lifecycle_intents: &mut Vec<GraphIntent>,
        data_dir: PathBuf,
    ) -> Result<(), String> {
        // Preflight the new directory first so failed switches are non-destructive.
        crate::persistence::GraphStore::open(data_dir.clone()).map_err(|e| e.to_string())?;
        let snapshot_interval_secs = graph_app.snapshot_interval_secs();

        lifecycle_intents.extend(webview_controller::close_all_webviews(graph_app, window));
        Self::reset_runtime_webview_state(
            tiles_tree,
            tile_rendering_contexts,
            tile_favicon_textures,
            favicon_textures,
        );

        graph_app.switch_persistence_dir(data_dir)?;
        if let Some(secs) = snapshot_interval_secs {
            graph_app.set_snapshot_interval_secs(secs)?;
        }
        *tiles_tree = Self::restore_tiles_tree_from_persistence(graph_app);
        Ok(())
    }

    fn parse_data_dir_input(raw: &str) -> Option<PathBuf> {
        let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(PathBuf::from(trimmed))
    }

    fn reset_runtime_webview_state(
        tiles_tree: &mut Tree<TileKind>,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        tile_favicon_textures: &mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
        favicon_textures: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    ) {
        tile_rendering_contexts.clear();
        tile_favicon_textures.clear();
        favicon_textures.clear();
        Self::remove_all_webview_tiles(tiles_tree);
    }

    fn collect_tile_invariant_violations(
        tiles_tree: &Tree<TileKind>,
        graph_app: &GraphBrowserApp,
        tile_rendering_contexts: &HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    ) -> Vec<String> {
        let mut violations = Vec::new();
        for node_key in Self::all_webview_tile_nodes(tiles_tree) {
            if graph_app.graph.get_node(node_key).is_none() {
                violations.push(format!(
                    "tile/webview desync: tile has stale node key {}",
                    node_key.index()
                ));
                continue;
            }
            if graph_app.get_webview_for_node(node_key).is_none() {
                violations.push(format!(
                    "tile/webview desync: node {} is missing webview mapping",
                    node_key.index()
                ));
            }
            if !tile_rendering_contexts.contains_key(&node_key) {
                violations.push(format!(
                    "tile/context desync: node {} is missing rendering context",
                    node_key.index()
                ));
            }
        }
        violations
    }

    pub(crate) fn is_graph_view(&self) -> bool {
        !self.has_active_webview_tile()
    }

    /// Set the RunningAppState reference for webview creation
    pub(crate) fn set_state(&mut self, state: Rc<RunningAppState>) {
        self.state = Some(state);
    }

    pub(crate) fn surrender_focus(&self) {
        self.context.egui_ctx.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
    }

    pub(crate) fn on_window_event(
        &mut self,
        winit_window: &Window,
        event: &WindowEvent,
    ) -> EventResponse {
        let mut response = self.context.on_window_event(winit_window, event);

        // When no WebView tile is active, consume user input events so they
        // never reach an inactive/hidden WebView.
        if !self.has_active_webview_tile() {
            match event {
                WindowEvent::KeyboardInput { .. }
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::MouseInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::Touch(_)
                | WindowEvent::PinchGesture { .. } => {
                    response.consumed = true;
                },
                _ => {},
            }
        }

        response
    }

    /// The height of the top toolbar of this user inteface ie the distance from the top of the
    /// window to the position of the `WebView`.
    pub(crate) fn toolbar_height(&self) -> Length<f32, DeviceIndependentPixel> {
        self.toolbar_height
    }

    pub(crate) fn webview_at_point(
        &self,
        point: Point2D<f32, DeviceIndependentPixel>,
    ) -> Option<(WebViewId, Point2D<f32, DeviceIndependentPixel>)> {
        let cursor = pos2(point.x, point.y);
        for tile_id in self.tiles_tree.active_tiles() {
            let Some(Tile::Pane(TileKind::WebView(node_key))) = self.tiles_tree.tiles.get(tile_id)
            else {
                continue;
            };
            let Some(rect) = self.tiles_tree.tiles.rect(tile_id) else {
                continue;
            };
            if !rect.contains(cursor) {
                continue;
            }
            let Some(webview_id) = self.graph_app.get_webview_for_node(*node_key) else {
                continue;
            };
            let local = Point2D::new(point.x - rect.min.x, point.y - rect.min.y);
            return Some((webview_id, local));
        }
        None
    }

    pub(crate) fn focused_webview_id(&self) -> Option<WebViewId> {
        Self::focused_webview_id_for_tree(
            &self.tiles_tree,
            &self.graph_app,
            self.focused_webview_hint,
        )
    }

    pub(crate) fn focused_tile_webview_id(&self) -> Option<WebViewId> {
        self.focused_webview_id()
    }

    pub(crate) fn set_focused_webview_id(&mut self, webview_id: WebViewId) {
        self.focused_webview_hint = Some(webview_id);
    }

    pub(crate) fn location_has_focus(&self) -> bool {
        self.context.egui_ctx.memory(|m| {
            m.focused()
                .is_some_and(|focused| focused == egui::Id::new("location_input"))
        })
    }

    pub(crate) fn request_location_submit(&mut self) {
        self.location_submitted = true;
    }

    pub(crate) fn egui_wants_keyboard_input(&self) -> bool {
        self.context.egui_ctx.wants_keyboard_input()
    }

    /// Create a frameless button with square sizing, as used in the toolbar.
    fn toolbar_button(text: &str) -> egui::Button<'_> {
        egui::Button::new(text)
            .frame(false)
            .min_size(Vec2 { x: 20.0, y: 20.0 })
    }

    /// Update the user interface, but do not paint the updated state.
    pub(crate) fn update(
        &mut self,
        state: &RunningAppState,
        window: &ServoShellWindow,
        headed_window: &headed_window::HeadedWindow,
    ) {
        // Note: We need Rc<RunningAppState> for webview creation, but this method
        // is called from trait methods that only provide &RunningAppState.
        // The caller should have Rc available at the call site.
        self.rendering_context
            .make_current()
            .expect("Could not make RenderingContext current");
        self.ensure_tiles_tree_root();
        debug_assert!(
            self.tiles_tree.root().is_some(),
            "tile tree root must exist before rendering"
        );
        let Self {
            rendering_context,
            window_rendering_context,
            context,
            tiles_tree,
            toolbar_height,
            location,
            location_dirty,
            location_submitted,
            show_clear_data_confirm,
            show_data_dir_dialog,
            data_dir_input,
            data_dir_status,
            show_persistence_settings_dialog,
            snapshot_interval_input,
            persistence_settings_status,
            favicon_textures,
            graph_app,
            tile_rendering_contexts,
            tile_favicon_textures,
            thumbnail_capture_tx,
            thumbnail_capture_rx,
            thumbnail_capture_in_flight,
            graph_search_open,
            graph_search_query,
            graph_search_filter_mode,
            graph_search_matches,
            graph_search_active_match_index,
            webview_creation_backpressure,
            state: app_state,
            focused_webview_hint,
            ..
        } = self;

        let winit_window = headed_window.winit_window();
        context.run(winit_window, |ctx| {
            let mut frame_intents = Vec::new();
            let mut post_render_intents = Vec::new();
            let mut pending_open_child_webviews = Vec::new();
            let mut open_selected_tile_after_intents: Option<OpenWebviewTileMode> = None;

            frame_intents.extend(load_pending_thumbnail_results(
                graph_app,
                window,
                thumbnail_capture_rx,
                thumbnail_capture_in_flight,
            ));
            let (semantic_intents, created_children, responsive_webviews) =
                graph_intents_from_pending_semantic_events(window);
            frame_intents.extend(semantic_intents);
            pending_open_child_webviews.extend(created_children);
            frame_intents.extend(load_pending_favicons(ctx, window, graph_app, favicon_textures));
            request_pending_thumbnail_captures(
                graph_app,
                window,
                thumbnail_capture_tx,
                thumbnail_capture_in_flight,
            );

            let graph_search_available = Self::active_webview_tile_node(tiles_tree).is_none();
            if !graph_search_available && *graph_search_open {
                *graph_search_open = false;
                graph_search_query.clear();
                graph_search_matches.clear();
                *graph_search_active_match_index = None;
                *graph_search_filter_mode = false;
                graph_app.egui_state_dirty = true;
            }

            let search_shortcut_pressed = ctx.input(|i| {
                if cfg!(target_os = "macos") {
                    i.modifiers.command && i.key_pressed(Key::F)
                } else {
                    i.modifiers.ctrl && i.key_pressed(Key::F)
                }
            });
            let mut focus_graph_search_field = false;
            let mut focus_location_field_for_search = false;
            if graph_search_available && search_shortcut_pressed {
                // Omnibox-first graph search: Ctrl+F focuses the location bar
                // with an `@` query prefix instead of opening a separate dialog.
                *graph_search_open = false;
                if !location.starts_with('@') {
                    *location = "@".to_string();
                }
                *location_dirty = true;
                focus_location_field_for_search = true;
            }

            let mut suppress_toggle_view = false;
            if *graph_search_open {
                refresh_graph_search_matches(
                    graph_app,
                    graph_search_query,
                    graph_search_matches,
                    graph_search_active_match_index,
                );

                if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
                    step_graph_search_active_match(
                        graph_search_matches,
                        graph_search_active_match_index,
                        1,
                    );
                }
                if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
                    step_graph_search_active_match(
                        graph_search_matches,
                        graph_search_active_match_index,
                        -1,
                    );
                }
                if ctx.input(|i| i.key_pressed(Key::Enter))
                    && let Some(node_key) = active_graph_search_match(
                        graph_search_matches,
                        *graph_search_active_match_index,
                    )
                {
                    frame_intents.push(GraphIntent::SelectNode {
                        key: node_key,
                        multi_select: false,
                    });
                }
                if ctx.input(|i| i.key_pressed(Key::Escape)) {
                    suppress_toggle_view = true;
                    if graph_search_query.trim().is_empty() {
                        *graph_search_open = false;
                        *graph_search_filter_mode = false;
                    } else {
                        graph_search_query.clear();
                    }
                    refresh_graph_search_matches(
                        graph_app,
                        graph_search_query,
                        graph_search_matches,
                        graph_search_active_match_index,
                    );
                    graph_app.egui_state_dirty = true;
                }
            }

            // Handle keyboard shortcuts regardless of view (e.g., toggle view)
            let mut keyboard_actions = input::collect_actions(ctx);
            if suppress_toggle_view {
                keyboard_actions.toggle_view = false;
            }
            if keyboard_actions.toggle_view {
                Self::toggle_tile_view(
                    tiles_tree,
                    graph_app,
                    window,
                    app_state,
                    rendering_context,
                    window_rendering_context,
                    tile_rendering_contexts,
                    &responsive_webviews,
                    webview_creation_backpressure,
                    &mut frame_intents,
                );
                keyboard_actions.toggle_view = false;
            }
            if keyboard_actions.delete_selected {
                let nodes_to_close: Vec<_> = graph_app.selected_nodes.iter().copied().collect();
                frame_intents.extend(webview_controller::close_webviews_for_nodes(
                    graph_app,
                    &nodes_to_close,
                    window,
                ));
            }
            if keyboard_actions.clear_graph {
                frame_intents.extend(webview_controller::close_all_webviews(graph_app, window));
                Self::reset_runtime_webview_state(
                    tiles_tree,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    favicon_textures,
                );
            }
            frame_intents.extend(input::intents_from_actions(&keyboard_actions));

            // Check which view mode we're in (used throughout rendering)
            let active_webview_node = Self::active_webview_tile_node(tiles_tree);
            let has_webview_tiles = Self::has_any_webview_tiles_in(tiles_tree);
            let is_graph_view = !has_webview_tiles;
            // Fullscreen mitigation: a persistent origin strip is rendered below.
            if winit_window.fullscreen().is_none() {
                let frame = egui::Frame::default()
                    .fill(ctx.style().visuals.window_fill)
                    .inner_margin(4.0);
                TopBottomPanel::top("toolbar").frame(frame).show(ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let back_button =
                                ui.add_enabled(self.can_go_back, Gui::toolbar_button("<"));
                            back_button.widget_info(|| {
                                let mut info = WidgetInfo::new(WidgetType::Button);
                                info.label = Some("Back".into());
                                info
                            });
                            if back_button.clicked() {
                                *location_dirty = false;
                                if let Some(webview_id) =
                                    Self::nav_target_webview_id(graph_app, active_webview_node)
                                    && let Some(webview) = window.webview_by_id(webview_id)
                                {
                                    webview.go_back(1);
                                    window.set_needs_update();
                                }
                            }

                            let forward_button =
                                ui.add_enabled(self.can_go_forward, Gui::toolbar_button(">"));
                            forward_button.widget_info(|| {
                                let mut info = WidgetInfo::new(WidgetType::Button);
                                info.label = Some("Forward".into());
                                info
                            });
                            if forward_button.clicked() {
                                *location_dirty = false;
                                if let Some(webview_id) =
                                    Self::nav_target_webview_id(graph_app, active_webview_node)
                                    && let Some(webview) = window.webview_by_id(webview_id)
                                {
                                    webview.go_forward(1);
                                    window.set_needs_update();
                                }
                            }

                            let reload_button = ui.add(Gui::toolbar_button("R"));
                            reload_button.widget_info(|| {
                                let mut info = WidgetInfo::new(WidgetType::Button);
                                info.label = Some("Reload".into());
                                info
                            });
                            if reload_button.clicked() {
                                *location_dirty = false;
                                if let Some(webview_id) =
                                    Self::nav_target_webview_id(graph_app, active_webview_node)
                                    && let Some(webview) = window.webview_by_id(webview_id)
                                {
                                    webview.reload();
                                    window.set_needs_update();
                                }
                            }
                            ui.add_space(2.0);

                            ui.allocate_ui_with_layout(
                                ui.available_size(),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let mut experimental_preferences_enabled =
                                        state.experimental_preferences_enabled();
                                    let prefs_toggle = ui
                                        .toggle_value(&mut experimental_preferences_enabled, "Exp")
                                        .on_hover_text("Enable experimental prefs");
                                    prefs_toggle.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Enable experimental preferences".into());
                                        info.selected = Some(experimental_preferences_enabled);
                                        info
                                    });
                                    if prefs_toggle.clicked() {
                                        state.set_experimental_preferences_enabled(
                                            experimental_preferences_enabled,
                                        );
                                        *location_dirty = false;
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::ReloadAll,
                                        );
                                    }

                                    // Graph/Detail mode toggle button (tile-driven).
                                    let has_webview_tiles =
                                        Self::has_any_webview_tiles_in(tiles_tree);
                                    let (view_icon, view_tooltip) = if has_webview_tiles {
                                        ("Graph", "Switch to Graph View")
                                    } else {
                                        ("Detail", "Switch to Detail View")
                                    };
                                    let view_toggle_button = ui
                                        .add(Gui::toolbar_button(view_icon))
                                        .on_hover_text(view_tooltip);
                                    view_toggle_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Toggle View".into());
                                        info
                                    });
                                    if view_toggle_button.clicked() {
                                        Self::toggle_tile_view(
                                            tiles_tree,
                                            graph_app,
                                            window,
                                            app_state,
                                            rendering_context,
                                            window_rendering_context,
                                            tile_rendering_contexts,
                                            &responsive_webviews,
                                            webview_creation_backpressure,
                                            &mut frame_intents,
                                        );
                                    }

                                    let data_dir_button = ui
                                        .add(Gui::toolbar_button("Dir"))
                                        .on_hover_text("Switch graph data directory");
                                    data_dir_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Switch graph data directory".into());
                                        info
                                    });
                                    if data_dir_button.clicked() {
                                        *show_data_dir_dialog = true;
                                    }

                                    let persistence_settings_button = ui
                                        .add(Gui::toolbar_button("Cfg"))
                                        .on_hover_text("Persistence settings");
                                    persistence_settings_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Persistence settings".into());
                                        info
                                    });
                                    if persistence_settings_button.clicked() {
                                        *show_persistence_settings_dialog = true;
                                    }

                                    let clear_data_button = ui
                                        .add(Gui::toolbar_button("Clr"))
                                        .on_hover_text("Clear graph and saved data");
                                    clear_data_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Clear graph and saved data".into());
                                        info
                                    });
                                    if clear_data_button.clicked() {
                                        *show_clear_data_confirm = true;
                                    }
                                    let physics_button = ui
                                        .add(Gui::toolbar_button("Phys"))
                                        .on_hover_text("Show/hide physics settings panel");
                                    if physics_button.clicked() {
                                        frame_intents.push(GraphIntent::TogglePhysicsPanel);
                                    }

                                    let new_node_button = ui
                                        .add(Gui::toolbar_button("Node+"))
                                        .on_hover_text("Create a new graph node");
                                    if new_node_button.clicked() {
                                        frame_intents.push(GraphIntent::CreateNodeNearCenter);
                                    }
                                    let new_tab_button = ui
                                        .add(Gui::toolbar_button("Tab+"))
                                        .on_hover_text(
                                            "Create a new node and open it as a tab in this graph window",
                                        );
                                    if new_tab_button.clicked() {
                                        frame_intents.push(GraphIntent::CreateNodeNearCenter);
                                        open_selected_tile_after_intents =
                                            Some(OpenWebviewTileMode::Tab);
                                    }
                                    let split_button = ui
                                        .add(Gui::toolbar_button("Split+"))
                                        .on_hover_text(
                                            "Create a new node and open it in a split pane",
                                        );
                                    if split_button.clicked() {
                                        frame_intents.push(GraphIntent::CreateNodeNearCenter);
                                        open_selected_tile_after_intents =
                                            Some(OpenWebviewTileMode::SplitHorizontal);
                                    }

                                    let location_id = egui::Id::new("location_input");
                                    let location_field = ui.add_sized(
                                        ui.available_size(),
                                        egui::TextEdit::singleline(location)
                                            .id(location_id)
                                            .hint_text("Search or enter address"),
                                    );

                                    if location_field.changed() {
                                        *location_dirty = true;
                                    }
                                    // Handle adddress bar shortcut.
                                    if focus_location_field_for_search || ui.input(|i| {
                                        if cfg!(target_os = "macos") {
                                            i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                        } else {
                                            i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                                || i.clone().consume_key(Modifiers::ALT, Key::D)
                                        }
                                    }) {
                                        // The focus request immediately makes gained_focus return true.
                                        location_field.request_focus();
                                    }
                                    // Select address bar text when it's focused (click or shortcut).
                                    if location_field.gained_focus() {
                                        if let Some(mut state) =
                                            TextEditState::load(ui.ctx(), location_id)
                                        {
                                            // Select the whole input.
                                            state.cursor.set_char_range(Some(CCursorRange::two(
                                                CCursor::new(0),
                                                CCursor::new(location.len()),
                                            )));
                                            state.store(ui.ctx(), location_id);
                                        }
                                    }
                                    // Submit immediately on Enter while focused.
                                    // Keep a deferred lost-focus fallback for any backend/event
                                    // ordering where Enter is observed in a different frame.
                                    let enter_while_focused = location_field.has_focus()
                                        && ui.input(|i| i.key_pressed(Key::Enter));
                                    if enter_while_focused {
                                        *location_submitted = true;
                                    }
                                    let should_submit_now = enter_while_focused
                                        || *location_submitted
                                        || (location_field.lost_focus()
                                            && ui.input(|i| i.key_pressed(Key::Enter)));
                                    if should_submit_now {
                                        *location_submitted = false;
                                        let focused_webview_id = active_webview_node
                                            .and_then(|key| graph_app.get_webview_for_node(key));
                                        let submit_result =
                                            webview_controller::handle_address_bar_submit_intents(
                                                graph_app,
                                                location,
                                                is_graph_view,
                                                focused_webview_id,
                                                window,
                                                &state.servoshell_preferences.searchpage,
                                            );
                                        frame_intents.extend(submit_result.intents);
                                        let submit_outcome = submit_result.outcome;
                                        if submit_outcome.mark_clean {
                                            *location_dirty = false;
                                            if submit_outcome.open_selected_tile {
                                                open_selected_tile_after_intents =
                                                    Some(OpenWebviewTileMode::Tab);
                                            }
                                        }
                                    }
                                },
                            );
                        },
                    );
                });

                if *graph_search_open && is_graph_view {
                    egui::Window::new("Graph Search")
                        .id(egui::Id::new("graph_search_window"))
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 52.0])
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                let search_id = egui::Id::new("graph_search_input");
                                let search_field = ui.add(
                                    egui::TextEdit::singleline(graph_search_query)
                                        .id(search_id)
                                        .desired_width(280.0)
                                        .hint_text("Find node title or URL"),
                                );
                                if focus_graph_search_field {
                                    search_field.request_focus();
                                    focus_graph_search_field = false;
                                }
                                if search_field.changed() {
                                    refresh_graph_search_matches(
                                        graph_app,
                                        graph_search_query,
                                        graph_search_matches,
                                        graph_search_active_match_index,
                                    );
                                    graph_app.egui_state_dirty = true;
                                }
                                if ui.checkbox(graph_search_filter_mode, "Filter").changed() {
                                    graph_app.egui_state_dirty = true;
                                }
                                if ui.button("Clear").clicked() {
                                    graph_search_query.clear();
                                    refresh_graph_search_matches(
                                        graph_app,
                                        graph_search_query,
                                        graph_search_matches,
                                        graph_search_active_match_index,
                                    );
                                    graph_app.egui_state_dirty = true;
                                }
                            });
                            let active_display = graph_search_active_match_index
                                .map(|idx| idx + 1)
                                .unwrap_or(0);
                            ui.label(format!(
                                "{} matches | active {}",
                                graph_search_matches.len(),
                                active_display
                            ));
                });
            } else {
                // Fullscreen anti-phishing mitigation: keep a minimal always-visible
                // origin strip so users can verify current page context.
                let fullscreen_url = active_webview_node
                    .and_then(|key| graph_app.graph.get_node(key).map(|node| node.url.clone()))
                    .unwrap_or_else(|| "about:blank".to_string());
                let frame = egui::Frame::default()
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 25, 220))
                    .inner_margin(4.0);
                TopBottomPanel::top("fullscreen_origin_strip")
                    .frame(frame)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Fullscreen");
                            ui.separator();
                            ui.label(fullscreen_url);
                            ui.separator();
                            ui.label("Press Esc to exit");
                        });
                    });
            }

            };

            if *show_clear_data_confirm {
                egui::Window::new("Clear Saved Graph Data?")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("This clears all graph nodes and saved graph data.");
                        ui.label("This action cannot be undone.");
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                *show_clear_data_confirm = false;
                            }
                            if ui.button("Clear Data").clicked() {
                                frame_intents.extend(webview_controller::close_all_webviews(
                                    graph_app, window,
                                ));
                                Self::reset_runtime_webview_state(
                                    tiles_tree,
                                    tile_rendering_contexts,
                                    tile_favicon_textures,
                                    favicon_textures,
                                );
                                graph_app.clear_graph_and_persistence();
                                *location_dirty = false;
                                *show_clear_data_confirm = false;
                            }
                        });
                    });
            }

            // The toolbar height is where the Context’s available rect starts.
            // For reasons that are unclear, the TopBottomPanel’s ui cursor exceeds this by one egui
            // point, but the Context is correct and the TopBottomPanel is wrong.
            if *show_data_dir_dialog {
                egui::Window::new("Switch Graph Data Directory")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Enter a directory path to load/save graph data.");
                        ui.add(
                            egui::TextEdit::singleline(data_dir_input)
                                .desired_width(480.0)
                                .hint_text("C:\\path\\to\\graph_data"),
                        );
                        if let Some(message) = data_dir_status.as_deref() {
                            ui.label(message);
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                *show_data_dir_dialog = false;
                                *data_dir_status = None;
                            }
                            if ui.button("Switch").clicked() {
                                let Some(target_dir) = Self::parse_data_dir_input(data_dir_input)
                                else {
                                    *data_dir_status =
                                        Some("Enter a non-empty directory path.".to_string());
                                    return;
                                };
                                match Self::switch_persistence_store(
                                    graph_app,
                                    window,
                                    tiles_tree,
                                    tile_rendering_contexts,
                                    tile_favicon_textures,
                                    favicon_textures,
                                    &mut frame_intents,
                                    target_dir.clone(),
                                ) {
                                    Ok(()) => {
                                        *location = graph_app
                                            .graph
                                            .nodes()
                                            .next()
                                            .map(|(_, node)| node.url.clone())
                                            .unwrap_or_default();
                                        *snapshot_interval_input = graph_app
                                            .snapshot_interval_secs()
                                            .unwrap_or(
                                                crate::persistence::DEFAULT_SNAPSHOT_INTERVAL_SECS,
                                            )
                                            .to_string();
                                        *location_dirty = false;
                                        *location_submitted = false;
                                        *show_data_dir_dialog = false;
                                        *data_dir_input = target_dir.display().to_string();
                                        *data_dir_status = None;
                                    },
                                    Err(e) => {
                                        *data_dir_status =
                                            Some(format!("Failed to switch data directory: {e}"));
                                    },
                                }
                            }
                        });
                    });
            }

            if *show_persistence_settings_dialog {
                egui::Window::new("Persistence Settings")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Snapshot interval (seconds):");
                        ui.add(
                            egui::TextEdit::singleline(snapshot_interval_input)
                                .desired_width(180.0)
                                .hint_text("300"),
                        );
                        if let Some(message) = persistence_settings_status.as_deref() {
                            ui.label(message);
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Close").clicked() {
                                *show_persistence_settings_dialog = false;
                                *persistence_settings_status = None;
                            }
                            if ui.button("Apply").clicked() {
                                let raw = snapshot_interval_input.trim();
                                let parsed_secs = raw.parse::<u64>();
                                match parsed_secs {
                                    Ok(secs) => match graph_app.set_snapshot_interval_secs(secs) {
                                        Ok(()) => {
                                            *snapshot_interval_input = secs.to_string();
                                            *persistence_settings_status =
                                                Some("Snapshot interval updated.".to_string());
                                        },
                                        Err(e) => {
                                            *persistence_settings_status =
                                                Some(format!("Failed to update interval: {e}"));
                                        },
                                    },
                                    Err(_) => {
                                        *persistence_settings_status =
                                            Some("Enter a valid positive integer.".to_string());
                                    },
                                }
                            }
                        });
                    });
            }

            // Phase 1: apply semantic/UI intents before lifecycle reconciliation.
            if !frame_intents.is_empty() {
                graph_app.apply_intents(std::mem::take(&mut frame_intents));
            }

            if let Some(open_mode) = open_selected_tile_after_intents
                && let Some(node_key) = graph_app.get_single_selected_node()
            {
                Self::open_or_focus_webview_tile_with_mode(tiles_tree, node_key, open_mode);
                // A node created from omnibar submit in detail-mode fallback starts as Cold.
                // Promote it so lifecycle reconciliation can instantiate its webview.
                frame_intents.push(GraphIntent::PromoteNodeToActive { key: node_key });
            }
            for child_webview_id in pending_open_child_webviews {
                if let Some(node_key) = graph_app.get_node_for_webview(child_webview_id) {
                    Self::open_or_focus_webview_tile(tiles_tree, node_key);
                }
            }
            if graph_app.graph.node_count() == 0 {
                graph_app.active_webview_nodes.clear();
                webview_creation_backpressure.clear();
                Self::reset_runtime_webview_state(
                    tiles_tree,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    favicon_textures,
                );
            }

            Self::prune_stale_webview_tiles(
                tiles_tree,
                graph_app,
                window,
                tile_rendering_contexts,
                &mut frame_intents,
            );
            tile_favicon_textures
                .retain(|node_key, _| graph_app.graph.get_node(*node_key).is_some());

            let has_webview_tiles = Self::has_any_webview_tiles_in(tiles_tree);
            if has_webview_tiles {
                frame_intents.extend(webview_controller::sync_to_graph_intents(graph_app, window));
                Self::reconcile_webview_creation_backpressure(
                    graph_app,
                    window,
                    &responsive_webviews,
                    webview_creation_backpressure,
                    &mut frame_intents,
                );

                // Keep WebView/context mappings complete for all tile nodes (not only visible ones).
                for node_key in Self::all_webview_tile_nodes(tiles_tree) {
                    Self::ensure_webview_for_node(
                        graph_app,
                        window,
                        app_state,
                        rendering_context,
                        window_rendering_context,
                        tile_rendering_contexts,
                        node_key,
                        &responsive_webviews,
                        webview_creation_backpressure,
                        &mut frame_intents,
                    );
                }
            } else {
                webview_creation_backpressure.clear();
            }

            // Phase 2: apply lifecycle/mapping intents emitted during reconciliation.
            // No rendering should occur between this apply and the subsequent render path.
            if !frame_intents.is_empty() {
                graph_app.apply_intents(std::mem::take(&mut frame_intents));
            }

            #[cfg(debug_assertions)]
            {
                for violation in Self::collect_tile_invariant_violations(
                    tiles_tree,
                    graph_app,
                    tile_rendering_contexts,
                ) {
                    warn!("{violation}");
                }
            }

            let has_webview_tiles = Self::has_any_webview_tiles_in(tiles_tree);
            let is_graph_view = !has_webview_tiles;

            *toolbar_height = Length::new(ctx.available_rect().min.y);

            // Check periodic persistence snapshot
            graph_app.check_periodic_snapshot();

            // Keep embedder dialogs active regardless of tile/view mode.
            // Pass precomputed toolbar height to avoid re-borrowing `headed_window.gui`
            // while this update holds a mutable borrow.
            let focused_dialog_webview =
                Self::focused_webview_id_for_tree(tiles_tree, graph_app, *focused_webview_hint);
            headed_window.for_each_active_dialog(
                window,
                focused_dialog_webview,
                *toolbar_height,
                |dialog| dialog.update(ctx),
            );

            // Tile-driven rendering path (single active render path).
            if is_graph_view || has_webview_tiles {
                // === TILE VIEW: render graph pane and any open webview panes ===
                let mut pending_open_nodes = Vec::new();
                let mut pending_closed_nodes = Vec::new();
                let search_query_active = !graph_search_query.trim().is_empty();
                let search_matches: HashSet<NodeKey> =
                    graph_search_matches.iter().copied().collect();
                let active_search_match = active_graph_search_match(
                    graph_search_matches,
                    *graph_search_active_match_index,
                );
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::from_rgb(20, 20, 25)))
                    .show(ctx, |ui| {
                        let mut behavior = GraphshellTileBehavior::new(
                            graph_app,
                            tile_favicon_textures,
                            &search_matches,
                            active_search_match,
                            *graph_search_filter_mode,
                            search_query_active,
                        );
                        tiles_tree.ui(&mut behavior, ui);
                        pending_open_nodes.extend(behavior.take_pending_open_nodes());
                        pending_closed_nodes.extend(behavior.take_pending_closed_nodes());
                        post_render_intents.extend(behavior.take_pending_graph_intents());
                    });
                for node_key in pending_open_nodes {
                    Self::open_or_focus_webview_tile(tiles_tree, node_key);
                }
                for node_key in pending_closed_nodes {
                    Self::close_webview_for_node(
                        graph_app,
                        window,
                        tile_rendering_contexts,
                        node_key,
                        &mut post_render_intents,
                    );
                }

                // Keep runtime webviews aligned with the current tile tree.
                let tile_nodes = Self::all_webview_tile_nodes(tiles_tree);
                let mapped_nodes: Vec<_> = graph_app
                    .webview_node_mappings()
                    .map(|(_, node_key)| node_key)
                    .collect();
                for node_key in mapped_nodes {
                    if !tile_nodes.contains(&node_key) {
                        Self::close_webview_for_node(
                            graph_app,
                            window,
                            tile_rendering_contexts,
                            node_key,
                            &mut post_render_intents,
                        );
                    }
                }

                let active_tile_rects = Self::active_webview_tile_rects(tiles_tree);
                for (node_key, _) in active_tile_rects.iter().copied() {
                    Self::ensure_webview_for_node(
                        graph_app,
                        window,
                        app_state,
                        rendering_context,
                        window_rendering_context,
                        tile_rendering_contexts,
                        node_key,
                        &responsive_webviews,
                        webview_creation_backpressure,
                        &mut post_render_intents,
                    );
                }
                if let Some((node_key, _)) = active_tile_rects.first().copied()
                    && let Some(wv_id) = graph_app.get_webview_for_node(node_key)
                {
                    *focused_webview_hint = Some(wv_id);
                    window.activate_webview(wv_id);
                }

                // Composite all visible WebView tiles with per-node rendering contexts.
                let scale =
                    Scale::<_, DeviceIndependentPixel, DevicePixel>::new(ctx.pixels_per_point());
                for (node_key, tile_rect) in active_tile_rects {
                    let size = Size2D::new(tile_rect.width(), tile_rect.height()) * scale;
                    let target_size = PhysicalSize::new(
                        size.width.max(1.0).round() as u32,
                        size.height.max(1.0).round() as u32,
                    );

                    let Some(render_context) = tile_rendering_contexts.get(&node_key).cloned()
                    else {
                        continue;
                    };

                    if render_context.size() != target_size {
                        render_context.resize(target_size);
                    }

                    let Some(webview_id) = graph_app.get_webview_for_node(node_key) else {
                        continue;
                    };
                    let Some(webview) = window.webview_by_id(webview_id) else {
                        continue;
                    };
                    if webview.size() != size {
                        webview.resize(target_size);
                    }

                    if let Err(e) = render_context.make_current() {
                        warn!("Failed to make tile rendering context current: {e:?}");
                        continue;
                    }
                    render_context.prepare_for_rendering();
                    webview.paint();
                    render_context.present();

                    if let Some(render_to_parent) = render_context.render_to_parent_callback() {
                        ctx.layer_painter(LayerId::background()).add(PaintCallback {
                            rect: tile_rect,
                            callback: Arc::new(CallbackFn::new(move |info, painter| {
                                let clip = info.viewport_in_pixels();
                                let rect_in_parent = Rect::new(
                                    Point2D::new(clip.left_px, clip.from_bottom_px),
                                    Size2D::new(clip.width_px, clip.height_px),
                                );
                                render_to_parent(painter.gl(), rect_in_parent)
                            })),
                        });
                    }
                }
            }

            if !post_render_intents.is_empty() {
                graph_app.apply_intents(post_render_intents);
            }

            // Render floating panels (available in both views)
            render::render_physics_panel(ctx, graph_app);
            render::render_help_panel(ctx, graph_app);
        });
    }

    fn ensure_tiles_tree_root(&mut self) {
        if self.tiles_tree.root().is_none() {
            let graph_tile_id = self.tiles_tree.tiles.insert_pane(TileKind::Graph);
            self.tiles_tree.root = Some(graph_tile_id);
        }
    }

    fn has_any_webview_tiles_in(tiles_tree: &Tree<TileKind>) -> bool {
        tiles_tree
            .tiles
            .iter()
            .any(|(_, tile)| matches!(tile, Tile::Pane(TileKind::WebView(_))))
    }

    fn nav_target_webview_id(
        graph_app: &GraphBrowserApp,
        active_webview_node: Option<crate::graph::NodeKey>,
    ) -> Option<WebViewId> {
        active_webview_node.and_then(|node_key| graph_app.get_webview_for_node(node_key))
    }

    fn preferred_detail_node(graph_app: &GraphBrowserApp) -> Option<crate::graph::NodeKey> {
        graph_app
            .get_single_selected_node()
            .or_else(|| graph_app.graph.nodes().next().map(|(key, _)| key))
    }

    fn toggle_tile_view(
        tiles_tree: &mut Tree<TileKind>,
        graph_app: &mut GraphBrowserApp,
        window: &ServoShellWindow,
        app_state: &Option<Rc<RunningAppState>>,
        base_rendering_context: &Rc<OffscreenRenderingContext>,
        window_rendering_context: &Rc<WindowRenderingContext>,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        responsive_webviews: &HashSet<WebViewId>,
        webview_creation_backpressure: &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
        lifecycle_intents: &mut Vec<GraphIntent>,
    ) {
        if Self::has_any_webview_tiles_in(tiles_tree) {
            let webview_nodes = Self::all_webview_tile_nodes(tiles_tree);
            let tile_ids: Vec<_> = tiles_tree
                .tiles
                .iter()
                .filter_map(|(tile_id, tile)| match tile {
                    Tile::Pane(TileKind::WebView(_)) => Some(*tile_id),
                    _ => None,
                })
                .collect();
            for tile_id in tile_ids {
                tiles_tree.remove_recursively(tile_id);
            }
            for node_key in webview_nodes {
                Self::close_webview_for_node(
                    graph_app,
                    window,
                    tile_rendering_contexts,
                    node_key,
                    lifecycle_intents,
                );
            }
        } else if let Some(node_key) = Self::preferred_detail_node(graph_app) {
            Self::open_or_focus_webview_tile(tiles_tree, node_key);
            Self::ensure_webview_for_node(
                graph_app,
                window,
                app_state,
                base_rendering_context,
                window_rendering_context,
                tile_rendering_contexts,
                node_key,
                responsive_webviews,
                webview_creation_backpressure,
                lifecycle_intents,
            );
        }
    }

    fn has_active_webview_tile(&self) -> bool {
        self.tiles_tree.active_tiles().into_iter().any(|tile_id| {
            matches!(
                self.tiles_tree.tiles.get(tile_id),
                Some(Tile::Pane(TileKind::WebView(_)))
            )
        })
    }

    fn active_webview_tile_node(tiles_tree: &Tree<TileKind>) -> Option<crate::graph::NodeKey> {
        tiles_tree.active_tiles().into_iter().find_map(|tile_id| {
            match tiles_tree.tiles.get(tile_id) {
                Some(Tile::Pane(TileKind::WebView(node_key))) => Some(*node_key),
                _ => None,
            }
        })
    }

    fn focused_webview_id_for_tree(
        tiles_tree: &Tree<TileKind>,
        graph_app: &GraphBrowserApp,
        focused_hint: Option<WebViewId>,
    ) -> Option<WebViewId> {
        if let Some(hint) = focused_hint {
            let hint_is_active = tiles_tree.active_tiles().into_iter().any(|tile_id| {
                matches!(
                    tiles_tree.tiles.get(tile_id),
                    Some(Tile::Pane(TileKind::WebView(node_key)))
                        if graph_app.get_webview_for_node(*node_key) == Some(hint)
                )
            });
            if hint_is_active {
                return Some(hint);
            }
        }

        Self::active_webview_tile_node(tiles_tree)
            .and_then(|node_key| graph_app.get_webview_for_node(node_key))
    }

    fn active_webview_tile_rects(
        tiles_tree: &Tree<TileKind>,
    ) -> Vec<(crate::graph::NodeKey, egui::Rect)> {
        let mut tile_rects = Vec::new();
        for tile_id in tiles_tree.active_tiles() {
            if let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles_tree.tiles.get(tile_id)
                && let Some(rect) = tiles_tree.tiles.rect(tile_id)
            {
                tile_rects.push((*node_key, rect));
            }
        }
        tile_rects
    }

    fn all_webview_tile_nodes(tiles_tree: &Tree<TileKind>) -> HashSet<crate::graph::NodeKey> {
        tiles_tree
            .tiles
            .iter()
            .filter_map(|(_, tile)| match tile {
                Tile::Pane(TileKind::WebView(node_key)) => Some(*node_key),
                _ => None,
            })
            .collect()
    }

    fn prune_stale_webview_tile_keys_only(
        tiles_tree: &mut Tree<TileKind>,
        graph_app: &GraphBrowserApp,
    ) {
        let stale_nodes: Vec<_> = Self::all_webview_tile_nodes(tiles_tree)
            .into_iter()
            .filter(|node_key| graph_app.graph.get_node(*node_key).is_none())
            .collect();
        for node_key in stale_nodes {
            Self::remove_webview_tile_for_node(tiles_tree, node_key);
        }
    }

    fn remove_all_webview_tiles(tiles_tree: &mut Tree<TileKind>) {
        let tile_ids: Vec<_> = tiles_tree
            .tiles
            .iter()
            .filter_map(|(tile_id, tile)| match tile {
                Tile::Pane(TileKind::WebView(_)) => Some(*tile_id),
                _ => None,
            })
            .collect();
        for tile_id in tile_ids {
            tiles_tree.remove_recursively(tile_id);
        }
    }

    fn remove_webview_tile_for_node(
        tiles_tree: &mut Tree<TileKind>,
        node_key: crate::graph::NodeKey,
    ) {
        let tile_ids: Vec<_> = tiles_tree
            .tiles
            .iter()
            .filter_map(|(tile_id, tile)| match tile {
                Tile::Pane(TileKind::WebView(key)) if *key == node_key => Some(*tile_id),
                _ => None,
            })
            .collect();
        for tile_id in tile_ids {
            tiles_tree.remove_recursively(tile_id);
        }
    }

    fn prune_stale_webview_tiles(
        tiles_tree: &mut Tree<TileKind>,
        graph_app: &mut GraphBrowserApp,
        window: &ServoShellWindow,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        lifecycle_intents: &mut Vec<GraphIntent>,
    ) {
        let stale_nodes: Vec<_> = Self::all_webview_tile_nodes(tiles_tree)
            .into_iter()
            .filter(|node_key| graph_app.graph.get_node(*node_key).is_none())
            .collect();

        for node_key in stale_nodes {
            Self::remove_webview_tile_for_node(tiles_tree, node_key);
            Self::close_webview_for_node(
                graph_app,
                window,
                tile_rendering_contexts,
                node_key,
                lifecycle_intents,
            );
        }
    }

    fn ensure_webview_for_node(
        graph_app: &mut GraphBrowserApp,
        window: &ServoShellWindow,
        app_state: &Option<Rc<RunningAppState>>,
        base_rendering_context: &Rc<OffscreenRenderingContext>,
        window_rendering_context: &Rc<WindowRenderingContext>,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        node_key: crate::graph::NodeKey,
        responsive_webviews: &HashSet<WebViewId>,
        webview_creation_backpressure: &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
        lifecycle_intents: &mut Vec<GraphIntent>,
    ) {
        let (Some(node), Some(running_state)) = (graph_app.graph.get_node(node_key), app_state.as_ref())
        else {
            webview_creation_backpressure.remove(&node_key);
            return;
        };
        if node.lifecycle != NodeLifecycle::Active {
            webview_creation_backpressure.remove(&node_key);
            return;
        }
        let node_url = node.url.clone();

        if let Some(existing_webview_id) = graph_app.get_webview_for_node(node_key) {
            if window.contains_webview(existing_webview_id) {
                if responsive_webviews.contains(&existing_webview_id)
                    && let Some(state) = webview_creation_backpressure.get_mut(&node_key)
                {
                    state.pending = None;
                    state.retry_count = 0;
                }
                return;
            }
            lifecycle_intents.push(GraphIntent::UnmapWebview {
                webview_id: existing_webview_id,
            });
        }

        let state = webview_creation_backpressure.entry(node_key).or_default();
        if state.pending.is_some() {
            return;
        }
        if state.retry_count >= WEBVIEW_CREATION_MAX_RETRIES {
            lifecycle_intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
            return;
        }

        let render_context = tile_rendering_contexts
            .entry(node_key)
            .or_insert_with(|| {
                Rc::new(window_rendering_context.offscreen_context(base_rendering_context.size()))
            })
            .clone();
        let url = Url::parse(&node_url).unwrap_or_else(|_| Url::parse("about:blank").unwrap());
        let webview =
            window.create_toplevel_webview_with_context(running_state.clone(), url, render_context);
        state.retry_count = state.retry_count.saturating_add(1);
        state.pending = Some(WebviewCreationProbe {
            webview_id: webview.id(),
            started_at: Instant::now(),
        });
        lifecycle_intents.extend([
            GraphIntent::MapWebviewToNode {
                webview_id: webview.id(),
                key: node_key,
            },
            GraphIntent::PromoteNodeToActive { key: node_key },
        ]);
    }

    fn reconcile_webview_creation_backpressure(
        graph_app: &GraphBrowserApp,
        window: &ServoShellWindow,
        responsive_webviews: &HashSet<WebViewId>,
        webview_creation_backpressure: &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
        lifecycle_intents: &mut Vec<GraphIntent>,
    ) {
        let tracked_nodes: Vec<NodeKey> = webview_creation_backpressure.keys().copied().collect();
        for node_key in tracked_nodes {
            let Some(node) = graph_app.graph.get_node(node_key) else {
                webview_creation_backpressure.remove(&node_key);
                continue;
            };
            if node.lifecycle != NodeLifecycle::Active {
                webview_creation_backpressure.remove(&node_key);
                continue;
            }

            let mut remove_state = false;
            if let Some(state) = webview_creation_backpressure.get_mut(&node_key)
                && let Some(probe) = state.pending
            {
                let contains_webview = window.contains_webview(probe.webview_id);
                let has_responsive_signal = responsive_webviews.contains(&probe.webview_id);
                match classify_webview_creation_probe(
                    probe.started_at.elapsed(),
                    contains_webview,
                    has_responsive_signal,
                ) {
                    WebviewCreationProbeOutcome::Confirmed => {
                        state.pending = None;
                        state.retry_count = 0;
                    },
                    WebviewCreationProbeOutcome::Pending => {},
                    WebviewCreationProbeOutcome::TimedOut => {
                        if contains_webview {
                            window.close_webview(probe.webview_id);
                        }
                        lifecycle_intents.push(GraphIntent::UnmapWebview {
                            webview_id: probe.webview_id,
                        });
                        state.pending = None;
                        if state.retry_count >= WEBVIEW_CREATION_MAX_RETRIES {
                            warn!(
                                "Demoting node {:?} after {} webview creation retries without confirmation",
                                node_key,
                                state.retry_count
                            );
                            lifecycle_intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
                            remove_state = true;
                        }
                    },
                }
            }
            if remove_state {
                webview_creation_backpressure.remove(&node_key);
            }
        }
    }

    fn close_webview_for_node(
        graph_app: &mut GraphBrowserApp,
        window: &ServoShellWindow,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        node_key: crate::graph::NodeKey,
        lifecycle_intents: &mut Vec<GraphIntent>,
    ) {
        if let Some(wv_id) = graph_app.get_webview_for_node(node_key) {
            window.close_webview(wv_id);
            lifecycle_intents.push(GraphIntent::UnmapWebview { webview_id: wv_id });
        }
        tile_rendering_contexts.remove(&node_key);
        lifecycle_intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
    }

    fn open_or_focus_webview_tile(
        tiles_tree: &mut Tree<TileKind>,
        node_key: crate::graph::NodeKey,
    ) {
        Self::open_or_focus_webview_tile_with_mode(tiles_tree, node_key, OpenWebviewTileMode::Tab);
    }

    fn open_or_focus_webview_tile_with_mode(
        tiles_tree: &mut Tree<TileKind>,
        node_key: crate::graph::NodeKey,
        mode: OpenWebviewTileMode,
    ) {
        if tiles_tree.make_active(
            |_, tile| matches!(tile, Tile::Pane(TileKind::WebView(key)) if *key == node_key),
        ) {
            return;
        }

        let webview_tile_id = tiles_tree.tiles.insert_pane(TileKind::WebView(node_key));
        let Some(root_id) = tiles_tree.root() else {
            tiles_tree.root = Some(webview_tile_id);
            return;
        };

        match mode {
            OpenWebviewTileMode::Tab => {
                if let Some(Tile::Container(Container::Tabs(tabs))) =
                    tiles_tree.tiles.get_mut(root_id)
                {
                    tabs.add_child(webview_tile_id);
                    tabs.set_active(webview_tile_id);
                    return;
                }

                let tabs_root = tiles_tree
                    .tiles
                    .insert_tab_tile(vec![root_id, webview_tile_id]);
                tiles_tree.root = Some(tabs_root);
            },
            OpenWebviewTileMode::SplitHorizontal => {
                let split_root = tiles_tree
                    .tiles
                    .insert_horizontal_tile(vec![root_id, webview_tile_id]);
                tiles_tree.root = Some(split_root);
            },
        }
    }

    /// Paint the GUI, as of the last update.
    pub(crate) fn paint(&mut self, window: &Window) {
        self.rendering_context
            .make_current()
            .expect("Could not make RenderingContext current");
        self.rendering_context
            .parent_context()
            .prepare_for_rendering();
        self.context.paint(window);
        self.rendering_context.parent_context().present();
    }

    /// Updates the location field from the given [`RunningAppState`], unless the user has started
    /// editing it without clicking Go, returning true iff it has changed (needing an egui update).
    fn update_location_in_toolbar(&mut self, window: &ServoShellWindow) -> bool {
        // User edited without clicking Go?
        if self.location_dirty {
            return false;
        }

        // In graph context, show the selected node URL.
        if !Self::has_any_webview_tiles_in(&self.tiles_tree) {
            if let Some(key) = self.graph_app.get_single_selected_node() {
                if let Some(node) = self.graph_app.graph.get_node(key) {
                    if node.url != self.location {
                        self.location = node.url.clone();
                        return true;
                    }
                }
            }
            return false;
        }

        let current_url_string = self
            .focused_webview_id()
            .and_then(|id| window.webview_by_id(id))
            .and_then(|webview| Some(webview.url()?.to_string()));
        match current_url_string {
            Some(location) if location != self.location => {
                self.location = location.to_owned();
                true
            },
            _ => false,
        }
    }

    fn update_load_status(&mut self, window: &ServoShellWindow) -> bool {
        let state_status = self
            .focused_webview_id()
            .and_then(|id| window.webview_by_id(id))
            .map(|webview| webview.load_status())
            .unwrap_or(LoadStatus::Complete);
        let old_status = std::mem::replace(&mut self.load_status, state_status);
        let status_changed = old_status != self.load_status;

        // When the load status changes, we want the new changes to the URL to start
        // being reflected in the location bar.
        if status_changed {
            self.location_dirty = false;
        }

        status_changed
    }

    fn update_status_text(&mut self, window: &ServoShellWindow) -> bool {
        let state_status = self
            .focused_webview_id()
            .and_then(|id| window.webview_by_id(id))
            .and_then(|webview| webview.status_text());
        let old_status = std::mem::replace(&mut self.status_text, state_status);
        old_status != self.status_text
    }

    fn update_can_go_back_and_forward(&mut self, window: &ServoShellWindow) -> bool {
        let (can_go_back, can_go_forward) = self
            .focused_webview_id()
            .and_then(|id| window.webview_by_id(id))
            .map(|webview| (webview.can_go_back(), webview.can_go_forward()))
            .unwrap_or((false, false));
        let old_can_go_back = std::mem::replace(&mut self.can_go_back, can_go_back);
        let old_can_go_forward = std::mem::replace(&mut self.can_go_forward, can_go_forward);
        old_can_go_back != self.can_go_back || old_can_go_forward != self.can_go_forward
    }

    /// Updates all fields taken from the given [`ServoShellWindow`], such as the location field.
    /// Returns true iff the egui needs an update.
    pub(crate) fn update_webview_data(&mut self, window: &ServoShellWindow) -> bool {
        // Note: We must use the "bitwise OR" (|) operator here instead of "logical OR" (||)
        //       because logical OR would short-circuit if any of the functions return true.
        //       We want to ensure that all functions are called. The "bitwise OR" operator
        //       does not short-circuit.
        self.update_load_status(window)
            | self.update_location_in_toolbar(window)
            | self.update_status_text(window)
            | self.update_can_go_back_and_forward(window)
    }

    /// Returns true if a redraw is required after handling the provided event.
    pub(crate) fn handle_accesskit_event(
        &mut self,
        event: &egui_winit::accesskit_winit::WindowEvent,
    ) -> bool {
        match event {
            egui_winit::accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.context.egui_ctx.enable_accesskit();
                true
            },
            egui_winit::accesskit_winit::WindowEvent::ActionRequested(req) => {
                self.context
                    .egui_winit
                    .on_accesskit_action_request(req.clone());
                true
            },
            egui_winit::accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                self.context.egui_ctx.disable_accesskit();
                false
            },
        }
    }

    pub(crate) fn set_zoom_factor(&self, factor: f32) {
        let clamped = if factor.is_finite() {
            factor.clamp(0.25, 4.0)
        } else {
            1.0
        };
        self.context.egui_ctx.set_zoom_factor(clamped);
    }

    pub(crate) fn notify_accessibility_tree_update(
        &mut self,
        webview_id: WebViewId,
        _tree_update: accesskit::TreeUpdate,
    ) {
        self.webview_accessibility_updates_dropped += 1;
        if !self.webview_accessibility_warned {
            self.webview_accessibility_warned = true;
            warn!(
                "WebView accessibility update dropped for {:?}: no embedder bridge available yet (issue #41930)",
                webview_id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_tiles::{Container, Tile, Tiles, Tree};

    /// Create a unique WebViewId for testing.
    fn test_webview_id() -> servo::WebViewId {
        thread_local! {
            static NS_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        NS_INSTALLED.with(|cell| {
            if !cell.get() {
                base::id::PipelineNamespace::install(base::id::PipelineNamespaceId(44));
                cell.set(true);
            }
        });
        servo::WebViewId::new(base::id::PainterId::next())
    }

    fn tree_with_graph_root() -> Tree<TileKind> {
        let mut tiles = Tiles::default();
        let graph_tile_id = tiles.insert_pane(TileKind::Graph);
        Tree::new("test_tree", graph_tile_id, tiles)
    }

    fn webview_tile_count(tiles_tree: &Tree<TileKind>, node_key: NodeKey) -> usize {
        tiles_tree
            .tiles
            .iter()
            .filter(
                |(_, tile)| matches!(tile, Tile::Pane(TileKind::WebView(key)) if *key == node_key),
            )
            .count()
    }

    #[test]
    fn test_open_webview_tile_creates_tabs_container() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(1);

        Gui::open_or_focus_webview_tile(&mut tree, node_key);

        assert!(Gui::has_any_webview_tiles_in(&tree));
        let root_id = tree.root().expect("root tile should exist");
        match tree.tiles.get(root_id) {
            Some(Tile::Container(Container::Tabs(tabs))) => {
                assert_eq!(tabs.children.len(), 2);
            },
            _ => panic!("expected tabs container root"),
        }
    }

    #[test]
    fn test_open_duplicate_tile_focuses_existing() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(7);

        Gui::open_or_focus_webview_tile(&mut tree, node_key);
        Gui::open_or_focus_webview_tile(&mut tree, node_key);

        assert_eq!(webview_tile_count(&tree, node_key), 1);
    }

    #[test]
    fn test_open_webview_tile_split_creates_horizontal_root() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(42);

        Gui::open_or_focus_webview_tile_with_mode(
            &mut tree,
            node_key,
            OpenWebviewTileMode::SplitHorizontal,
        );

        let root_id = tree.root().expect("root tile should exist");
        match tree.tiles.get(root_id) {
            Some(Tile::Container(Container::Linear(linear))) => {
                assert_eq!(linear.children.len(), 2);
            },
            _ => panic!("expected horizontal split container root"),
        }
    }

    #[test]
    fn test_close_last_webview_tile_leaves_graph_only() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(3);
        Gui::open_or_focus_webview_tile(&mut tree, node_key);

        Gui::remove_all_webview_tiles(&mut tree);

        assert!(!Gui::has_any_webview_tiles_in(&tree));
        let has_graph_pane = tree
            .tiles
            .iter()
            .any(|(_, tile)| matches!(tile, Tile::Pane(TileKind::Graph)));
        assert!(has_graph_pane);
    }

    #[test]
    fn test_all_webview_tile_nodes_tracks_correctly() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(1);
        let b = NodeKey::new(2);
        Gui::open_or_focus_webview_tile(&mut tree, a);
        Gui::open_or_focus_webview_tile(&mut tree, b);

        let nodes = Gui::all_webview_tile_nodes(&tree);
        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&a));
        assert!(nodes.contains(&b));
    }

    #[test]
    fn test_nav_target_webview_id_uses_active_tile_node_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let active_node = app.add_node_and_sync("https://active.example".into(), Point2D::new(0.0, 0.0));
        let other_node = app.add_node_and_sync("https://other.example".into(), Point2D::new(10.0, 0.0));
        let active_webview = test_webview_id();
        let other_webview = test_webview_id();
        app.map_webview_to_node(active_webview, active_node);
        app.map_webview_to_node(other_webview, other_node);

        let target = Gui::nav_target_webview_id(&app, Some(active_node));
        assert_eq!(target, Some(active_webview));
    }

    #[test]
    fn test_nav_target_webview_id_none_without_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let node = app.add_node_and_sync("https://unmapped.example".into(), Point2D::new(0.0, 0.0));
        assert_eq!(Gui::nav_target_webview_id(&app, Some(node)), None);
        assert_eq!(Gui::nav_target_webview_id(&app, None), None);
    }

    #[test]
    fn test_focused_webview_id_for_tree_prefers_active_hint() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("https://b.example".into(), Point2D::new(10.0, 0.0));
        let a_id = test_webview_id();
        let b_id = test_webview_id();
        app.map_webview_to_node(a_id, a);
        app.map_webview_to_node(b_id, b);

        let mut tiles = Tiles::default();
        let a_tile = tiles.insert_pane(TileKind::WebView(a));
        let b_tile = tiles.insert_pane(TileKind::WebView(b));
        let root = tiles.insert_horizontal_tile(vec![a_tile, b_tile]);
        let tree = Tree::new("focus_hint_test", root, tiles);

        let focused = Gui::focused_webview_id_for_tree(&tree, &app, Some(b_id));
        assert_eq!(focused, Some(b_id));
    }

    #[test]
    fn test_open_or_focus_sets_active_tile_to_target_node() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(10);
        let b = NodeKey::new(11);
        Gui::open_or_focus_webview_tile(&mut tree, a);
        Gui::open_or_focus_webview_tile(&mut tree, b);

        assert_eq!(Gui::active_webview_tile_node(&tree), Some(b));

        Gui::open_or_focus_webview_tile(&mut tree, a);
        assert_eq!(Gui::active_webview_tile_node(&tree), Some(a));
        assert_eq!(webview_tile_count(&tree, a), 1);
        assert_eq!(webview_tile_count(&tree, b), 1);
    }

    #[test]
    fn test_remove_webview_tile_for_node_preserves_other_tiles() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(20);
        let b = NodeKey::new(21);
        Gui::open_or_focus_webview_tile(&mut tree, a);
        Gui::open_or_focus_webview_tile(&mut tree, b);

        Gui::remove_webview_tile_for_node(&mut tree, a);
        let nodes = Gui::all_webview_tile_nodes(&tree);
        assert!(!nodes.contains(&a));
        assert!(nodes.contains(&b));
    }

    #[test]
    fn test_stale_node_cleanup_removes_tile() {
        let mut app = GraphBrowserApp::new_for_testing();
        let alive_key =
            app.add_node_and_sync("https://alive.example".into(), Point2D::new(0.0, 0.0));
        let stale_key = NodeKey::new(9999);
        let mut tree = tree_with_graph_root();
        Gui::open_or_focus_webview_tile(&mut tree, alive_key);
        Gui::open_or_focus_webview_tile(&mut tree, stale_key);

        Gui::prune_stale_webview_tile_keys_only(&mut tree, &app);
        let nodes = Gui::all_webview_tile_nodes(&tree);
        assert!(nodes.contains(&alive_key));
        assert!(!nodes.contains(&stale_key));
    }

    #[test]
    fn test_tile_layout_serde_roundtrip() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(5);
        let b = NodeKey::new(6);
        Gui::open_or_focus_webview_tile(&mut tree, a);
        Gui::open_or_focus_webview_tile(&mut tree, b);

        let json = serde_json::to_string(&tree).expect("serialize tree");
        let restored: Tree<TileKind> = serde_json::from_str(&json).expect("deserialize tree");
        let nodes = Gui::all_webview_tile_nodes(&restored);

        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&a));
        assert!(nodes.contains(&b));
    }

    #[test]
    fn test_invariant_check_detects_desync() {
        let mut app = GraphBrowserApp::new_for_testing();
        let node_key = app.add_node_and_sync("https://example.test".into(), Point2D::new(0.0, 0.0));
        let mut tree = tree_with_graph_root();
        Gui::open_or_focus_webview_tile(&mut tree, node_key);

        let contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>> = HashMap::new();
        let violations = Gui::collect_tile_invariant_violations(&tree, &app, &contexts);

        assert!(
            violations
                .iter()
                .any(|v| v.contains("missing webview mapping"))
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("missing rendering context"))
        );
    }

    #[test]
    fn test_refresh_graph_search_matches_updates_active_index() {
        let mut app = GraphBrowserApp::new_for_testing();
        let github = app.add_node_and_sync("https://github.com".into(), Point2D::new(0.0, 0.0));
        let _example = app.add_node_and_sync("https://example.com".into(), Point2D::new(10.0, 0.0));

        let mut matches = Vec::new();
        let mut active = None;
        refresh_graph_search_matches(&app, "gthub", &mut matches, &mut active);

        assert_eq!(matches.first().copied(), Some(github));
        assert_eq!(active, Some(0));

        refresh_graph_search_matches(&app, "", &mut matches, &mut active);
        assert!(matches.is_empty());
        assert_eq!(active, None);
    }

    #[test]
    fn test_step_graph_search_active_match_wraps() {
        let matches = vec![NodeKey::new(1), NodeKey::new(2), NodeKey::new(3)];
        let mut active = Some(2);
        step_graph_search_active_match(&matches, &mut active, 1);
        assert_eq!(active, Some(0));

        step_graph_search_active_match(&matches, &mut active, -1);
        assert_eq!(active, Some(2));
    }

    #[test]
    fn test_active_graph_search_match_returns_current_key() {
        let matches = vec![NodeKey::new(10), NodeKey::new(11)];
        assert_eq!(
            active_graph_search_match(&matches, Some(1)),
            Some(NodeKey::new(11))
        );
        assert_eq!(active_graph_search_match(&matches, Some(2)), None);
        assert_eq!(active_graph_search_match(&matches, None), None);
    }

    #[test]
    fn test_parse_data_dir_input_trims_quotes_and_whitespace() {
        let parsed = Gui::parse_data_dir_input("  \"C:\\\\tmp\\\\graph data\"  ")
            .expect("should parse quoted path");
        assert_eq!(parsed, PathBuf::from("C:\\tmp\\graph data"));

        let parsed_single = Gui::parse_data_dir_input(" 'C:\\\\tmp\\\\graph' ")
            .expect("should parse single-quoted path");
        assert_eq!(parsed_single, PathBuf::from("C:\\tmp\\graph"));
    }

    #[test]
    fn test_parse_data_dir_input_empty_is_none() {
        assert!(Gui::parse_data_dir_input("").is_none());
        assert!(Gui::parse_data_dir_input("   ").is_none());
        assert!(Gui::parse_data_dir_input("\"\"").is_none());
    }

    #[test]
    fn test_graph_intents_from_semantic_events_preserves_order_and_variants() {
        let w1 = test_webview_id();
        let w2 = test_webview_id();
        let w3 = test_webview_id();
        let events = vec![
            GraphSemanticEvent::UrlChanged {
                webview_id: w1,
                new_url: "https://a.com".to_string(),
            },
            GraphSemanticEvent::HistoryChanged {
                webview_id: w2,
                entries: vec!["https://x.com".to_string()],
                current: 0,
            },
            GraphSemanticEvent::PageTitleChanged {
                webview_id: w1,
                title: Some("A".to_string()),
            },
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id: w1,
                child_webview_id: w3,
                initial_url: Some("https://child.com".to_string()),
            },
        ];

        let intents = graph_intents_from_semantic_events(events);
        assert_eq!(intents.len(), 4);
        assert!(matches!(
            &intents[0],
            GraphIntent::WebViewUrlChanged { webview_id, new_url }
                if *webview_id == w1 && new_url == "https://a.com"
        ));
        assert!(matches!(
            &intents[1],
            GraphIntent::WebViewHistoryChanged { webview_id, entries, current }
                if *webview_id == w2 && entries.len() == 1 && *current == 0
        ));
        assert!(matches!(
            &intents[2],
            GraphIntent::WebViewTitleChanged { webview_id, title }
                if *webview_id == w1 && title.as_deref() == Some("A")
        ));
        assert!(matches!(
            &intents[3],
            GraphIntent::WebViewCreated { parent_webview_id, child_webview_id, initial_url }
                if *parent_webview_id == w1
                    && *child_webview_id == w3
                    && initial_url.as_deref() == Some("https://child.com")
        ));
    }

    #[test]
    fn test_graph_intents_from_semantic_events_maps_webview_crashed() {
        let wv = test_webview_id();
        let events = vec![GraphSemanticEvent::WebViewCrashed {
            webview_id: wv,
            reason: "renderer panic".to_string(),
            has_backtrace: true,
        }];

        let intents = graph_intents_from_semantic_events(events);
        assert_eq!(intents.len(), 1);
        assert!(matches!(
            &intents[0],
            GraphIntent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace
            } if *webview_id == wv && reason == "renderer panic" && *has_backtrace
        ));
    }

    #[test]
    fn test_graph_intents_and_responsive_from_events_redirect_like_sequence_preserves_order() {
        let wv = test_webview_id();
        let events = vec![
            GraphSemanticEvent::UrlChanged {
                webview_id: wv,
                new_url: "https://redirect-a.example".into(),
            },
            GraphSemanticEvent::UrlChanged {
                webview_id: wv,
                new_url: "https://redirect-b.example".into(),
            },
            GraphSemanticEvent::PageTitleChanged {
                webview_id: wv,
                title: Some("Final".into()),
            },
            GraphSemanticEvent::HistoryChanged {
                webview_id: wv,
                entries: vec![
                    "https://start.example".into(),
                    "https://redirect-b.example".into(),
                ],
                current: 1,
            },
        ];

        let (intents, created_children, responsive_webviews) =
            graph_intents_and_responsive_from_events(events);

        assert!(created_children.is_empty());
        assert!(responsive_webviews.contains(&wv));
        assert_eq!(intents.len(), 4);
        assert!(matches!(
            &intents[0],
            GraphIntent::WebViewUrlChanged { webview_id, new_url }
                if *webview_id == wv && new_url == "https://redirect-a.example"
        ));
        assert!(matches!(
            &intents[1],
            GraphIntent::WebViewUrlChanged { webview_id, new_url }
                if *webview_id == wv && new_url == "https://redirect-b.example"
        ));
        assert!(matches!(
            &intents[2],
            GraphIntent::WebViewTitleChanged { webview_id, title }
                if *webview_id == wv && title.as_deref() == Some("Final")
        ));
        assert!(matches!(
            &intents[3],
            GraphIntent::WebViewHistoryChanged { webview_id, current, .. }
                if *webview_id == wv && *current == 1
        ));
    }

    #[test]
    fn test_graph_intents_and_responsive_from_events_create_new_is_prioritized() {
        let parent = test_webview_id();
        let child = test_webview_id();
        let events = vec![
            GraphSemanticEvent::UrlChanged {
                webview_id: parent,
                new_url: "https://parent.example".into(),
            },
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id: parent,
                child_webview_id: child,
                initial_url: Some("https://child.example".into()),
            },
            GraphSemanticEvent::PageTitleChanged {
                webview_id: parent,
                title: Some("Parent".into()),
            },
        ];

        let (intents, created_children, responsive_webviews) =
            graph_intents_and_responsive_from_events(events);

        assert_eq!(created_children, vec![child]);
        assert!(responsive_webviews.contains(&parent));
        assert!(responsive_webviews.contains(&child));
        assert_eq!(intents.len(), 3);
        assert!(matches!(
            &intents[0],
            GraphIntent::WebViewCreated { parent_webview_id, child_webview_id, .. }
                if *parent_webview_id == parent && *child_webview_id == child
        ));
        assert!(matches!(
            &intents[1],
            GraphIntent::WebViewUrlChanged { webview_id, .. } if *webview_id == parent
        ));
        assert!(matches!(
            &intents[2],
            GraphIntent::WebViewTitleChanged { webview_id, .. } if *webview_id == parent
        ));
    }

    #[test]
    fn test_semantic_events_to_intents_apply_to_graph_state() {
        let mut app = GraphBrowserApp::new_for_testing();
        let parent = app.add_node_and_sync("https://parent.com".into(), Point2D::new(10.0, 20.0));
        let parent_wv = test_webview_id();
        let child_wv = test_webview_id();
        app.map_webview_to_node(parent_wv, parent);

        let events = vec![
            GraphSemanticEvent::UrlChanged {
                webview_id: parent_wv,
                new_url: "https://parent-updated.com".into(),
            },
            GraphSemanticEvent::HistoryChanged {
                webview_id: parent_wv,
                entries: vec!["https://a.com".into(), "https://b.com".into()],
                current: 1,
            },
            GraphSemanticEvent::PageTitleChanged {
                webview_id: parent_wv,
                title: Some("Updated Parent".into()),
            },
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id: parent_wv,
                child_webview_id: child_wv,
                initial_url: Some("https://child.com".into()),
            },
        ];

        let intents = graph_intents_from_semantic_events(events);
        app.apply_intents(intents);

        let parent_node = app.graph.get_node(parent).unwrap();
        assert_eq!(parent_node.url, "https://parent-updated.com");
        assert_eq!(parent_node.title, "Updated Parent");
        assert_eq!(parent_node.history_entries.len(), 2);
        assert_eq!(parent_node.history_index, 1);

        let child = app.get_node_for_webview(child_wv).unwrap();
        assert_eq!(app.graph.get_node(child).unwrap().url, "https://child.com");
    }

    #[test]
    fn test_graph_intent_for_thumbnail_result_accepts_matching_url() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.add_node_and_sync("https://thumb.com".to_string(), Point2D::new(0.0, 0.0));
        let webview_id = test_webview_id();
        app.map_webview_to_node(webview_id, key);

        let result = ThumbnailCaptureResult {
            webview_id,
            requested_url: "https://thumb.com".to_string(),
            png_bytes: Some(vec![1, 2, 3, 4]),
            width: 2,
            height: 2,
        };

        let intent = graph_intent_for_thumbnail_result(&app, &result);
        assert!(matches!(
            intent,
            Some(GraphIntent::SetNodeThumbnail { key: k, width, height, .. })
                if k == key && width == 2 && height == 2
        ));
    }

    #[test]
    fn test_graph_intent_for_thumbnail_result_rejects_stale_or_empty() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.add_node_and_sync("https://thumb.com".to_string(), Point2D::new(0.0, 0.0));
        let webview_id = test_webview_id();
        app.map_webview_to_node(webview_id, key);

        let stale = ThumbnailCaptureResult {
            webview_id,
            requested_url: "https://other.com".to_string(),
            png_bytes: Some(vec![1, 2, 3, 4]),
            width: 2,
            height: 2,
        };
        assert!(graph_intent_for_thumbnail_result(&app, &stale).is_none());

        let empty_png = ThumbnailCaptureResult {
            webview_id,
            requested_url: "https://thumb.com".to_string(),
            png_bytes: None,
            width: 2,
            height: 2,
        };
        assert!(graph_intent_for_thumbnail_result(&app, &empty_png).is_none());
    }

    #[test]
    fn test_reset_runtime_webview_state_clears_tiles_and_texture_caches() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(77);
        Gui::open_or_focus_webview_tile(&mut tree, node_key);

        let mut tile_rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>> =
            HashMap::new();
        let mut tile_favicon_textures: HashMap<NodeKey, (u64, egui::TextureHandle)> =
            HashMap::new();
        let mut favicon_textures: HashMap<
            WebViewId,
            (egui::TextureHandle, egui::load::SizedTexture),
        > = HashMap::new();

        let ctx = egui::Context::default();
        let image = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255]);
        let handle = ctx.load_texture("test-reset-favicon", image, Default::default());
        tile_favicon_textures.insert(node_key, (123, handle.clone()));
        let wv_id = test_webview_id();
        let sized = egui::load::SizedTexture::new(handle.id(), egui::vec2(1.0, 1.0));
        favicon_textures.insert(wv_id, (handle, sized));

        Gui::reset_runtime_webview_state(
            &mut tree,
            &mut tile_rendering_contexts,
            &mut tile_favicon_textures,
            &mut favicon_textures,
        );

        assert!(!Gui::has_any_webview_tiles_in(&tree));
        assert!(tile_rendering_contexts.is_empty());
        assert!(tile_favicon_textures.is_empty());
        assert!(favicon_textures.is_empty());
    }

    #[test]
    fn test_classify_webview_creation_probe_confirms_on_responsive_signal() {
        let outcome = classify_webview_creation_probe(Duration::from_millis(10), false, true);
        assert_eq!(outcome, WebviewCreationProbeOutcome::Confirmed);
    }

    #[test]
    fn test_classify_webview_creation_probe_confirms_on_stable_live_webview() {
        let outcome = classify_webview_creation_probe(
            WEBVIEW_CREATION_CONFIRMATION_WINDOW + Duration::from_millis(1),
            true,
            false,
        );
        assert_eq!(outcome, WebviewCreationProbeOutcome::Confirmed);
    }

    #[test]
    fn test_classify_webview_creation_probe_times_out_without_confirmation() {
        let outcome = classify_webview_creation_probe(
            WEBVIEW_CREATION_TIMEOUT + Duration::from_millis(1),
            false,
            false,
        );
        assert_eq!(outcome, WebviewCreationProbeOutcome::TimedOut);
    }

    #[test]
    fn test_classify_webview_creation_probe_pending_before_timeout() {
        let outcome = classify_webview_creation_probe(Duration::from_millis(500), false, false);
        assert_eq!(outcome, WebviewCreationProbeOutcome::Pending);
    }
}

fn graph_intents_from_semantic_events(events: Vec<GraphSemanticEvent>) -> Vec<GraphIntent> {
    let mut intents = Vec::with_capacity(events.len());
    for event in events {
        match event {
            GraphSemanticEvent::UrlChanged {
                webview_id,
                new_url,
            } => intents.push(GraphIntent::WebViewUrlChanged {
                webview_id,
                new_url,
            }),
            GraphSemanticEvent::HistoryChanged {
                webview_id,
                entries,
                current,
            } => intents.push(GraphIntent::WebViewHistoryChanged {
                webview_id,
                entries,
                current,
            }),
            GraphSemanticEvent::PageTitleChanged { webview_id, title } => {
                intents.push(GraphIntent::WebViewTitleChanged { webview_id, title });
            },
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id,
                child_webview_id,
                initial_url,
            } => intents.push(GraphIntent::WebViewCreated {
                parent_webview_id,
                child_webview_id,
                initial_url,
            }),
            GraphSemanticEvent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace,
            } => intents.push(GraphIntent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace,
            }),
        }
    }
    intents
}

fn graph_intents_from_pending_semantic_events(
    window: &ServoShellWindow,
) -> (Vec<GraphIntent>, Vec<WebViewId>, HashSet<WebViewId>) {
    graph_intents_and_responsive_from_events(window.take_pending_graph_events())
}

fn graph_intents_and_responsive_from_events(
    events: Vec<GraphSemanticEvent>,
) -> (Vec<GraphIntent>, Vec<WebViewId>, HashSet<WebViewId>) {
    let mut create_events = Vec::new();
    let mut other_events = Vec::new();
    let mut created_child_webviews = Vec::new();
    let mut responsive_webviews = HashSet::new();

    for event in events {
        match &event {
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id,
                child_webview_id,
                ..
            } => {
                responsive_webviews.insert(*parent_webview_id);
                responsive_webviews.insert(*child_webview_id);
                created_child_webviews.push(*child_webview_id);
                create_events.push(event);
            },
            GraphSemanticEvent::UrlChanged { webview_id, .. } |
            GraphSemanticEvent::HistoryChanged { webview_id, .. } |
            GraphSemanticEvent::PageTitleChanged { webview_id, .. } => {
                responsive_webviews.insert(*webview_id);
                other_events.push(event);
            },
            GraphSemanticEvent::WebViewCrashed { .. } => {
                other_events.push(event);
            },
        }
    }

    let mut intents = graph_intents_from_semantic_events(create_events);
    intents.extend(graph_intents_from_semantic_events(other_events));
    (intents, created_child_webviews, responsive_webviews)
}

fn refresh_graph_search_matches(
    graph_app: &GraphBrowserApp,
    query: &str,
    matches: &mut Vec<NodeKey>,
    active_index: &mut Option<usize>,
) {
    if query.trim().is_empty() {
        matches.clear();
        *active_index = None;
        return;
    }

    *matches = fuzzy_match_node_keys(&graph_app.graph, query);
    if matches.is_empty() {
        *active_index = None;
    } else if active_index.is_none_or(|idx| idx >= matches.len()) {
        *active_index = Some(0);
    }
}

fn step_graph_search_active_match(
    matches: &[NodeKey],
    active_index: &mut Option<usize>,
    step: isize,
) {
    if matches.is_empty() {
        *active_index = None;
        return;
    }

    let current = active_index.unwrap_or(0) as isize;
    let len = matches.len() as isize;
    let next = (current + step).rem_euclid(len) as usize;
    *active_index = Some(next);
}

fn active_graph_search_match(matches: &[NodeKey], active_index: Option<usize>) -> Option<NodeKey> {
    let idx = active_index?;
    matches.get(idx).copied()
}

const NODE_THUMBNAIL_WIDTH: u32 = 256;
const NODE_THUMBNAIL_HEIGHT: u32 = 192;

struct ThumbnailCaptureResult {
    webview_id: WebViewId,
    requested_url: String,
    png_bytes: Option<Vec<u8>>,
    width: u32,
    height: u32,
}

fn request_pending_thumbnail_captures(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    result_tx: &Sender<ThumbnailCaptureResult>,
    in_flight: &mut HashSet<WebViewId>,
) {
    in_flight.retain(|id| window.contains_webview(*id));

    for id in window.take_pending_thumbnail_capture_requests() {
        if in_flight.contains(&id) {
            continue;
        }

        let Some(webview) = window.webview_by_id(id) else {
            continue;
        };
        let Some(node_key) = graph_app.get_node_for_webview(id) else {
            continue;
        };
        let Some(node) = graph_app.graph.get_node(node_key) else {
            continue;
        };

        let requested_url = node.url.clone();
        if requested_url.starts_with("about:blank") {
            continue;
        }
        let sender = result_tx.clone();
        in_flight.insert(id);
        webview.take_screenshot(None, move |result| {
            let (png_bytes, width, height) = match result {
                Ok(image) => {
                    let resized = DynamicImage::ImageRgba8(image)
                        .resize_to_fill(
                            NODE_THUMBNAIL_WIDTH,
                            NODE_THUMBNAIL_HEIGHT,
                            FilterType::Triangle,
                        )
                        .to_rgba8();
                    let (width, height) = resized.dimensions();
                    let mut cursor = Cursor::new(Vec::new());
                    let png_bytes = match DynamicImage::ImageRgba8(resized)
                        .write_to(&mut cursor, ImageFormat::Png)
                    {
                        Ok(()) => Some(cursor.into_inner()),
                        Err(error) => {
                            warn!("Could not encode thumbnail PNG for {id:?}: {error}");
                            None
                        },
                    };
                    (png_bytes, width, height)
                },
                Err(error) => {
                    warn!("Could not capture thumbnail for {id:?}: {error:?}");
                    (None, 0, 0)
                },
            };
            let _ = sender.send(ThumbnailCaptureResult {
                webview_id: id,
                requested_url,
                png_bytes,
                width,
                height,
            });
        });
    }
}

fn load_pending_thumbnail_results(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    result_rx: &Receiver<ThumbnailCaptureResult>,
    in_flight: &mut HashSet<WebViewId>,
) -> Vec<GraphIntent> {
    in_flight.retain(|id| window.contains_webview(*id));
    let mut intents = Vec::new();

    while let Ok(result) = result_rx.try_recv() {
        in_flight.remove(&result.webview_id);
        if let Some(intent) = graph_intent_for_thumbnail_result(graph_app, &result) {
            intents.push(intent);
        }
    }
    intents
}

fn graph_intent_for_thumbnail_result(
    graph_app: &GraphBrowserApp,
    result: &ThumbnailCaptureResult,
) -> Option<GraphIntent> {
    let node_key = graph_app.get_node_for_webview(result.webview_id)?;
    let node = graph_app.graph.get_node(node_key)?;
    if node.url != result.requested_url {
        return None;
    }
    let png_bytes = result.png_bytes.clone()?;
    Some(GraphIntent::SetNodeThumbnail {
        key: node_key,
        png_bytes,
        width: result.width,
        height: result.height,
    })
}

/// Convert a Servo image to RGBA8 bytes.
fn embedder_image_to_rgba(image: &Image) -> (usize, usize, Vec<u8>) {
    let width = image.width as usize;
    let height = image.height as usize;

    let data = match image.format {
        PixelFormat::K8 => image.data().iter().flat_map(|&v| [v, v, v, 255]).collect(),
        PixelFormat::KA8 => {
            // Convert to rgba
            image
                .data()
                .chunks_exact(2)
                .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
                .collect()
        },
        PixelFormat::RGB8 => image
            .data()
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        PixelFormat::RGBA8 => image.data().to_vec(),
        PixelFormat::BGRA8 => {
            // Convert from BGRA to RGBA
            image
                .data()
                .chunks_exact(4)
                .flat_map(|chunk| [chunk[2], chunk[1], chunk[0], chunk[3]])
                .collect()
        },
    };

    (width, height, data)
}

/// Uploads all favicons that have not yet been processed to the GPU.
fn load_pending_favicons(
    ctx: &egui::Context,
    window: &ServoShellWindow,
    graph_app: &GraphBrowserApp,
    texture_cache: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    for id in window.take_pending_favicon_loads() {
        let Some(webview) = window.webview_by_id(id) else {
            continue;
        };
        let Some(favicon) = webview.favicon() else {
            continue;
        };

        let (width, height, rgba) = embedder_image_to_rgba(&favicon);
        let egui_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        let handle = ctx.load_texture(format!("favicon-{id:?}"), egui_image, Default::default());
        let texture = egui::load::SizedTexture::new(
            handle.id(),
            egui::vec2(favicon.width as f32, favicon.height as f32),
        );

        // We don't need the handle anymore but we can't drop it either since that would cause
        // the texture to be freed.
        texture_cache.insert(id, (handle, texture));

        if let Some(node_key) = graph_app.get_node_for_webview(id) {
            intents.push(GraphIntent::SetNodeFavicon {
                key: node_key,
                rgba,
                width: width as u32,
                height: height as u32,
            });
        }
    }
    intents
}
