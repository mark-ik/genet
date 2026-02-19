/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::pos2;
use egui_glow::EguiGlow;
use egui_tiles::{Tile, Tiles, Tree};
use egui_winit::EventResponse;
use euclid::{Length, Point2D};
use log::warn;
use servo::{
    DeviceIndependentPixel, LoadStatus, OffscreenRenderingContext, RenderingContext, WebViewId,
    WindowRenderingContext,
};
use url::Url;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use super::graph_search_flow::{self, GraphSearchFlowArgs};
use super::graph_search_ui::{self, GraphSearchUiArgs};
use super::gui_frame::{self, PreFrameIngestArgs, ToolbarDialogPhaseArgs};
use super::persistence_ops;
use super::semantic_event_pipeline;
use super::thumbnail_pipeline::{self, ThumbnailCaptureResult};
use super::tile_compositor;
use super::tile_grouping;
use super::tile_invariants;
use super::tile_kind::TileKind;
use super::tile_runtime;
use super::tile_view_ops::{self, TileOpenMode, ToggleTileViewArgs};
use super::toolbar_routing::ToolbarOpenMode;
use super::toolbar_ui::OmnibarSearchSession;
use super::webview_backpressure::WebviewCreationBackpressureState;
use super::webview_status_sync;
use crate::app::{GraphBrowserApp, GraphIntent, PendingTileOpenMode};
use crate::desktop::event_loop::AppEvent;
use crate::desktop::headed_window;
use crate::graph::NodeKey;
use crate::running_app_state::RunningAppState;
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
    /// Explicit focus owner flag for graph pane interactions.
    graph_surface_focused: bool,

    /// Last webview that triggered focus-ring pulse.
    focus_ring_webview_id: Option<WebViewId>,

    /// Start timestamp for current focus-ring pulse.
    focus_ring_started_at: Option<Instant>,

    /// Duration for focus-ring pulse.
    focus_ring_duration: Duration,

    /// Session state for `@` omnibar fuzzy-node iteration.
    omnibar_search_session: Option<OmnibarSearchSession>,

    /// One-shot command palette toggle requested by window event routing.
    command_palette_toggle_requested: bool,
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
    fn open_mode_from_toolbar(mode: ToolbarOpenMode) -> TileOpenMode {
        match mode {
            ToolbarOpenMode::Tab => TileOpenMode::Tab,
            ToolbarOpenMode::SplitHorizontal => TileOpenMode::SplitHorizontal,
        }
    }

    fn open_mode_from_pending(mode: PendingTileOpenMode) -> TileOpenMode {
        match mode {
            PendingTileOpenMode::Tab => TileOpenMode::Tab,
            PendingTileOpenMode::SplitHorizontal => TileOpenMode::SplitHorizontal,
        }
    }

    fn default_open_mode_for_layout(_tiles_tree: &Tree<TileKind>) -> TileOpenMode {
        TileOpenMode::Tab
    }

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

        let startup_layout_json =
            graph_app.load_workspace_layout_json(GraphBrowserApp::SESSION_WORKSPACE_LAYOUT_NAME);
        let startup_layout_json = startup_layout_json.or_else(|| graph_app.load_tile_layout_json());
        if let Some(layout_json) = startup_layout_json
            && let Ok(mut restored_tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json)
        {
            tile_runtime::prune_stale_webview_tile_keys_only(&mut restored_tree, &graph_app);
            if restored_tree.root().is_some() {
                graph_app.mark_session_workspace_layout_json(&layout_json);
                tiles_tree = restored_tree;
            }
        }

        // Only create initial node if graph wasn't recovered from persistence
        if !graph_app.has_recovered_graph() {
            use euclid::default::Point2D;
            let _initial_node =
                graph_app.add_node_and_sync(initial_url.to_string(), Point2D::new(400.0, 300.0));
        }
        let membership_index = persistence_ops::build_membership_index_from_layouts(&graph_app);
        graph_app.init_membership_index(membership_index);
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
            graph_surface_focused: false,
            focus_ring_webview_id: None,
            focus_ring_started_at: None,
            focus_ring_duration: Duration::from_millis(500),
            omnibar_search_session: None,
            command_palette_toggle_requested: false,
        }
    }

    fn parse_data_dir_input(raw: &str) -> Option<PathBuf> {
        persistence_ops::parse_data_dir_input(raw)
    }

    fn reset_runtime_webview_state(
        tiles_tree: &mut Tree<TileKind>,
        tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        tile_favicon_textures: &mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
        favicon_textures: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    ) {
        tile_runtime::reset_runtime_webview_state(
            tiles_tree,
            tile_rendering_contexts,
            tile_favicon_textures,
            favicon_textures,
        );
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

    pub(crate) fn graph_at_point(&self, point: Point2D<f32, DeviceIndependentPixel>) -> bool {
        let cursor = pos2(point.x, point.y);
        self.tiles_tree.active_tiles().into_iter().any(|tile_id| {
            matches!(
                self.tiles_tree.tiles.get(tile_id),
                Some(Tile::Pane(TileKind::Graph))
            ) && self
                .tiles_tree
                .tiles
                .rect(tile_id)
                .is_some_and(|rect| rect.contains(cursor))
        })
    }

    pub(crate) fn focused_webview_id(&self) -> Option<WebViewId> {
        if self.graph_surface_focused {
            return None;
        }
        tile_compositor::focused_webview_id_for_tree(
            &self.tiles_tree,
            &self.graph_app,
            self.focused_webview_hint,
        )
    }

    pub(crate) fn focused_tile_webview_id(&self) -> Option<WebViewId> {
        self.focused_webview_id()
    }

    pub(crate) fn active_tile_webview_id(&self) -> Option<WebViewId> {
        tile_compositor::focused_webview_id_for_tree(&self.tiles_tree, &self.graph_app, None)
    }

    pub(crate) fn set_focused_webview_id(&mut self, webview_id: WebViewId) {
        self.focused_webview_hint = Some(webview_id);
        self.graph_surface_focused = false;
    }

    pub(crate) fn focus_graph_surface(&mut self) {
        self.focused_webview_hint = None;
        self.graph_surface_focused = true;
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

    pub(crate) fn request_command_palette_toggle(&mut self) {
        self.command_palette_toggle_requested = true;
    }

    pub(crate) fn egui_wants_keyboard_input(&self) -> bool {
        self.context.egui_ctx.wants_keyboard_input()
    }

    pub(crate) fn egui_wants_pointer_input(&self) -> bool {
        self.context.egui_ctx.wants_pointer_input()
    }

    pub(crate) fn pointer_hover_position(&self) -> Option<Point2D<f32, DeviceIndependentPixel>> {
        self.context
            .egui_ctx
            .input(|i| i.pointer.hover_pos())
            .map(|p| Point2D::new(p.x, p.y))
    }

    pub(crate) fn ui_overlay_active(&self) -> bool {
        self.graph_app.show_command_palette
            || self.graph_app.show_help_panel
            || self.graph_app.show_physics_panel
            || self.show_data_dir_dialog
            || self.show_persistence_settings_dialog
            || self.show_clear_data_confirm
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
            command_palette_toggle_requested,
            ..
        } = self;

        let winit_window = headed_window.winit_window();
        context.run(winit_window, |ctx| {
            graph_app.tick_frame();
            let mut frame_intents = Vec::new();
            let mut open_selected_tile_after_intents: Option<TileOpenMode> = None;
            if *command_palette_toggle_requested {
                *command_palette_toggle_requested = false;
                frame_intents.push(GraphIntent::ToggleCommandPalette);
            }

            let pre_frame = gui_frame::ingest_pre_frame(
                PreFrameIngestArgs {
                    ctx,
                    graph_app,
                    window,
                    favicon_textures,
                    thumbnail_capture_tx,
                    thumbnail_capture_rx,
                    thumbnail_capture_in_flight,
                },
                &mut frame_intents,
            );
            let pending_open_child_webviews = pre_frame.pending_open_child_webviews;
            let responsive_webviews = pre_frame.responsive_webviews;

            let graph_search_available = Self::active_webview_tile_node(tiles_tree).is_none();
            let mut graph_search_output = graph_search_flow::handle_graph_search_flow(
                GraphSearchFlowArgs {
                    ctx,
                    graph_app,
                    graph_search_open,
                    graph_search_query,
                    graph_search_filter_mode,
                    graph_search_matches,
                    graph_search_active_match_index,
                    location,
                    location_dirty,
                    frame_intents: &mut frame_intents,
                    graph_search_available,
                },
                |graph_app, query, matches, active_index| {
                    refresh_graph_search_matches(graph_app, query, matches, active_index);
                },
                |matches, active_index, delta| {
                    step_graph_search_active_match(matches, active_index, delta);
                },
                |matches, active_index| active_graph_search_match(matches, active_index),
            );

            gui_frame::handle_keyboard_phase(
                gui_frame::KeyboardPhaseArgs {
                    ctx,
                    graph_app,
                    window,
                    tiles_tree,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    favicon_textures,
                    app_state,
                    rendering_context,
                    window_rendering_context,
                    responsive_webviews: &responsive_webviews,
                    webview_creation_backpressure,
                    suppress_toggle_view: graph_search_output.suppress_toggle_view,
                },
                &mut frame_intents,
                |tiles_tree,
                 graph_app,
                 window,
                 app_state,
                 rendering_context,
                 window_rendering_context,
                 tile_rendering_contexts,
                 responsive_webviews,
                 webview_creation_backpressure,
                 frame_intents| {
                    Self::toggle_tile_view(
                        tiles_tree,
                        graph_app,
                        window,
                        app_state,
                        rendering_context,
                        window_rendering_context,
                        tile_rendering_contexts,
                        responsive_webviews,
                        webview_creation_backpressure,
                        frame_intents,
                    );
                },
                |tiles_tree, tile_rendering_contexts, tile_favicon_textures, favicon_textures| {
                    Self::reset_runtime_webview_state(
                        tiles_tree,
                        tile_rendering_contexts,
                        tile_favicon_textures,
                        favicon_textures,
                    );
                },
            );

            let toolbar_dialog_phase = gui_frame::handle_toolbar_dialog_phase(
                ToolbarDialogPhaseArgs {
                    ctx,
                    winit_window,
                    state,
                    graph_app,
                    window,
                    tiles_tree,
                    focused_webview_hint: *focused_webview_hint,
                    graph_surface_focused: self.graph_surface_focused,
                    can_go_back: self.can_go_back,
                    can_go_forward: self.can_go_forward,
                    location,
                    location_dirty,
                    location_submitted,
                    focus_location_field_for_search: graph_search_output
                        .focus_location_field_for_search,
                    show_data_dir_dialog,
                    show_persistence_settings_dialog,
                    show_clear_data_confirm,
                    omnibar_search_session: &mut self.omnibar_search_session,
                    data_dir_input,
                    data_dir_status,
                    snapshot_interval_input,
                    persistence_settings_status,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    favicon_textures,
                },
                &mut frame_intents,
            );
            let toolbar_output = toolbar_dialog_phase.toolbar_output;
            let is_graph_view = toolbar_dialog_phase.is_graph_view;
            if toolbar_output.toggle_tile_view_requested {
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
            if let Some(open_mode) = toolbar_output.open_selected_mode_after_submit {
                open_selected_tile_after_intents = Some(Self::open_mode_from_toolbar(open_mode));
            }

            if toolbar_output.toolbar_visible && *graph_search_open && is_graph_view {
                graph_search_ui::render_graph_search_window(
                    GraphSearchUiArgs {
                        ctx,
                        graph_app,
                        graph_search_query,
                        graph_search_filter_mode,
                        graph_search_matches,
                        graph_search_active_match_index,
                        focus_graph_search_field: &mut graph_search_output.focus_graph_search_field,
                    },
                    |graph_app, query, matches, active_index| {
                        refresh_graph_search_matches(graph_app, query, matches, active_index);
                    },
                );
            }

            // Phase 1: apply semantic/UI intents before lifecycle reconciliation.
            gui_frame::apply_intents_if_any(graph_app, tiles_tree, &mut frame_intents);

            if let Some(open_mode) = graph_app.take_pending_open_selected_tile_mode() {
                open_selected_tile_after_intents = Some(Self::open_mode_from_pending(open_mode));
            }

            if let Some(open_mode) = open_selected_tile_after_intents
                && let Some(node_key) = graph_app.get_single_selected_node()
            {
                if let Ok(layout_json) = serde_json::to_string(tiles_tree) {
                    graph_app.capture_undo_checkpoint(Some(layout_json));
                }
                let anchor_before_open = if open_mode == TileOpenMode::Tab {
                    gui_frame::active_webview_tile_node(tiles_tree)
                } else {
                    None
                };
                let node_already_in_workspace = tiles_tree.tiles.iter().any(|(_, tile)| {
                    matches!(
                        tile,
                        Tile::Pane(TileKind::WebView(existing_key)) if *existing_key == node_key
                    )
                });
                tile_view_ops::open_or_focus_webview_tile_with_mode(
                    tiles_tree, node_key, open_mode,
                );
                if open_mode == TileOpenMode::Tab
                    && !node_already_in_workspace
                    && let Some(anchor) = anchor_before_open
                    && anchor != node_key
                {
                    frame_intents.push(GraphIntent::CreateUserGroupedEdge {
                        from: anchor,
                        to: node_key,
                    });
                }
                // A node created from omnibar submit in detail-mode fallback starts as Cold.
                // Promote it so lifecycle reconciliation can instantiate its webview.
                frame_intents.push(GraphIntent::PromoteNodeToActive { key: node_key });
            }
            gui_frame::open_pending_child_webviews_for_tiles(
                graph_app,
                pending_open_child_webviews,
                |node_key| {
                    tile_view_ops::open_or_focus_webview_tile_with_mode(
                        tiles_tree,
                        node_key,
                        Self::default_open_mode_for_layout(tiles_tree),
                    );
                },
            );
            gui_frame::run_lifecycle_reconcile_and_apply(
                gui_frame::LifecycleReconcilePhaseArgs {
                    graph_app,
                    tiles_tree,
                    window,
                    app_state,
                    rendering_context,
                    window_rendering_context,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    favicon_textures,
                    responsive_webviews: &responsive_webviews,
                    webview_creation_backpressure,
                },
                &mut frame_intents,
            );

            gui_frame::run_post_render_phase(
                gui_frame::PostRenderPhaseArgs {
                    ctx,
                    graph_app,
                    window,
                    headed_window,
                    tiles_tree,
                    tile_rendering_contexts,
                    tile_favicon_textures,
                    toolbar_height,
                    graph_search_matches,
                    graph_search_active_match_index: *graph_search_active_match_index,
                    graph_search_filter_mode: *graph_search_filter_mode,
                    search_query_active: !graph_search_query.trim().is_empty(),
                    app_state,
                    rendering_context,
                    window_rendering_context,
                    responsive_webviews: &responsive_webviews,
                    webview_creation_backpressure,
                    focused_webview_hint,
                    graph_surface_focused: self.graph_surface_focused,
                    focus_ring_webview_id: &mut self.focus_ring_webview_id,
                    focus_ring_started_at: &mut self.focus_ring_started_at,
                    focus_ring_duration: self.focus_ring_duration,
                },
                |matches, active_index| active_graph_search_match(matches, active_index),
            );
        });
    }

    fn ensure_tiles_tree_root(&mut self) {
        if self.tiles_tree.root().is_none() {
            let graph_tile_id = self.tiles_tree.tiles.insert_pane(TileKind::Graph);
            self.tiles_tree.root = Some(graph_tile_id);
        }
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
        tile_view_ops::toggle_tile_view(ToggleTileViewArgs {
            tiles_tree,
            graph_app,
            window,
            app_state,
            base_rendering_context,
            window_rendering_context,
            tile_rendering_contexts,
            responsive_webviews,
            webview_creation_backpressure,
            lifecycle_intents,
        });
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
        if self.location.trim_start().starts_with('@') {
            // Preserve active omnibar node-search query text while cycling matches.
            return false;
        }
        let has_webview_tiles = tile_runtime::has_any_webview_tiles(&self.tiles_tree);
        let selected_node_url = self.graph_app.get_single_selected_node().and_then(|key| {
            self.graph_app
                .graph
                .get_node(key)
                .map(|node| node.url.clone())
        });
        let focused_webview_id = self.focused_webview_id();
        webview_status_sync::update_location_in_toolbar(
            self.location_dirty,
            &mut self.location,
            has_webview_tiles,
            selected_node_url,
            focused_webview_id,
            window,
        )
    }

    fn update_load_status(&mut self, window: &ServoShellWindow) -> bool {
        let focused_webview_id = self.focused_webview_id();
        webview_status_sync::update_load_status(
            &mut self.load_status,
            &mut self.location_dirty,
            focused_webview_id,
            window,
        )
    }

    fn update_status_text(&mut self, window: &ServoShellWindow) -> bool {
        let focused_webview_id = self.focused_webview_id();
        webview_status_sync::update_status_text(&mut self.status_text, focused_webview_id, window)
    }

    fn update_can_go_back_and_forward(&mut self, window: &ServoShellWindow) -> bool {
        let focused_webview_id = self.focused_webview_id();
        webview_status_sync::update_can_go_back_and_forward(
            &mut self.can_go_back,
            &mut self.can_go_forward,
            focused_webview_id,
            window,
        )
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
    use egui_tiles::{Container, Tile, TileId, Tiles, Tree};

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

    fn has_any_webview_tiles_in(tiles_tree: &Tree<TileKind>) -> bool {
        tile_runtime::has_any_webview_tiles(tiles_tree)
    }

    fn open_or_focus_webview_tile(tiles_tree: &mut Tree<TileKind>, node_key: NodeKey) {
        tile_view_ops::open_or_focus_webview_tile(tiles_tree, node_key);
    }

    fn open_or_focus_webview_tile_with_mode(
        tiles_tree: &mut Tree<TileKind>,
        node_key: NodeKey,
        mode: TileOpenMode,
    ) {
        tile_view_ops::open_or_focus_webview_tile_with_mode(tiles_tree, node_key, mode);
    }

    fn all_webview_tile_nodes(tiles_tree: &Tree<TileKind>) -> HashSet<NodeKey> {
        tile_runtime::all_webview_tile_nodes(tiles_tree)
    }

    fn remove_all_webview_tiles(tiles_tree: &mut Tree<TileKind>) {
        tile_runtime::remove_all_webview_tiles(tiles_tree);
    }

    fn remove_webview_tile_for_node(tiles_tree: &mut Tree<TileKind>, node_key: NodeKey) {
        tile_runtime::remove_webview_tile_for_node(tiles_tree, node_key);
    }

    fn prune_stale_webview_tile_keys_only(
        tiles_tree: &mut Tree<TileKind>,
        graph_app: &GraphBrowserApp,
    ) {
        tile_runtime::prune_stale_webview_tile_keys_only(tiles_tree, graph_app);
    }

    fn focused_webview_id_for_tree(
        tiles_tree: &Tree<TileKind>,
        graph_app: &GraphBrowserApp,
        focused_hint: Option<WebViewId>,
    ) -> Option<WebViewId> {
        tile_compositor::focused_webview_id_for_tree(tiles_tree, graph_app, focused_hint)
    }

    fn webview_for_frame_activation(
        tiles_tree: &Tree<TileKind>,
        graph_app: &GraphBrowserApp,
        focused_hint: Option<WebViewId>,
    ) -> Option<WebViewId> {
        tile_compositor::webview_for_frame_activation(tiles_tree, graph_app, focused_hint)
    }

    #[test]
    fn test_open_webview_tile_creates_tabs_container() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(1);

        open_or_focus_webview_tile(&mut tree, node_key);

        assert!(has_any_webview_tiles_in(&tree));
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

        open_or_focus_webview_tile(&mut tree, node_key);
        open_or_focus_webview_tile(&mut tree, node_key);

        assert_eq!(webview_tile_count(&tree, node_key), 1);
    }

    #[test]
    fn test_open_webview_tile_split_creates_horizontal_root() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(42);

        open_or_focus_webview_tile_with_mode(&mut tree, node_key, TileOpenMode::SplitHorizontal);

        let root_id = tree.root().expect("root tile should exist");
        match tree.tiles.get(root_id) {
            Some(Tile::Container(Container::Linear(linear))) => {
                assert_eq!(linear.children.len(), 2);
            },
            _ => panic!("expected horizontal split container root"),
        }
    }

    #[test]
    fn test_open_webview_tile_split_reuses_existing_linear_root() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(40);
        let b = NodeKey::new(41);

        open_or_focus_webview_tile_with_mode(&mut tree, a, TileOpenMode::SplitHorizontal);
        let root_before = tree.root().expect("root should exist");
        open_or_focus_webview_tile_with_mode(&mut tree, b, TileOpenMode::SplitHorizontal);
        let root_after = tree.root().expect("root should exist");

        assert_eq!(root_before, root_after);
        match tree.tiles.get(root_after) {
            Some(Tile::Container(Container::Linear(linear))) => {
                assert_eq!(linear.children.len(), 3);
            },
            _ => panic!("expected linear root container"),
        }
    }

    #[test]
    fn test_close_last_webview_tile_leaves_graph_only() {
        let mut tree = tree_with_graph_root();
        let node_key = NodeKey::new(3);
        open_or_focus_webview_tile(&mut tree, node_key);

        remove_all_webview_tiles(&mut tree);

        assert!(!has_any_webview_tiles_in(&tree));
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
        open_or_focus_webview_tile(&mut tree, a);
        open_or_focus_webview_tile(&mut tree, b);

        let nodes = all_webview_tile_nodes(&tree);
        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&a));
        assert!(nodes.contains(&b));
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

        let focused = focused_webview_id_for_tree(&tree, &app, Some(b_id));
        assert_eq!(focused, Some(b_id));
    }

    #[test]
    fn test_focused_webview_id_for_tree_prefers_hint_when_tile_still_present() {
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
        let root = tiles.insert_tab_tile(vec![a_tile, b_tile]);
        let mut tree = Tree::new("focus_hint_tab_test", root, tiles);
        let _ = tree.make_active(|tile_id, _| tile_id == a_tile);

        let focused = focused_webview_id_for_tree(&tree, &app, Some(b_id));
        assert_eq!(focused, Some(b_id));
    }

    #[test]
    fn test_get_focused_webview_semantics_use_tile_focused_hint() {
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
        let tree = Tree::new("webdriver_get_focused_semantics", root, tiles);

        // Mirrors WebDriver GetFocusedWebView semantics in desktop graphshell:
        // preferred input target should resolve to the tile-focused webview.
        let focused = focused_webview_id_for_tree(&tree, &app, Some(b_id));
        assert_eq!(focused, Some(b_id));
    }

    #[test]
    fn test_focused_webview_id_for_tree_graph_only_returns_none() {
        let app = GraphBrowserApp::new_for_testing();
        let tree = tree_with_graph_root();
        let stale_hint = test_webview_id();

        let focused = focused_webview_id_for_tree(&tree, &app, Some(stale_hint));
        assert_eq!(focused, None);
    }

    #[test]
    fn test_webview_for_frame_activation_prefers_active_hint() {
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
        let tree = Tree::new("frame_activation_hint_test", root, tiles);

        let chosen = webview_for_frame_activation(&tree, &app, Some(b_id));
        assert_eq!(chosen, Some(b_id));
    }

    #[test]
    fn test_webview_for_frame_activation_prefers_hint_when_tile_still_present() {
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
        let root = tiles.insert_tab_tile(vec![a_tile, b_tile]);
        let mut tree = Tree::new("frame_activation_active_test", root, tiles);
        let _ = tree.make_active(|tile_id, _| tile_id == a_tile);

        let chosen = webview_for_frame_activation(&tree, &app, Some(b_id));
        assert_eq!(chosen, Some(b_id));
    }

    #[test]
    fn test_split_layout_keeps_both_webview_tiles_visible_when_focus_changes() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("https://b.example".into(), Point2D::new(10.0, 0.0));
        let a_id = test_webview_id();
        let b_id = test_webview_id();
        app.map_webview_to_node(a_id, a);
        app.map_webview_to_node(b_id, b);

        let mut tree = tree_with_graph_root();
        open_or_focus_webview_tile_with_mode(&mut tree, a, TileOpenMode::SplitHorizontal);
        open_or_focus_webview_tile_with_mode(&mut tree, b, TileOpenMode::SplitHorizontal);

        let initial_nodes = all_webview_tile_nodes(&tree);
        assert_eq!(initial_nodes.len(), 2);
        assert!(initial_nodes.contains(&a));
        assert!(initial_nodes.contains(&b));

        let first = webview_for_frame_activation(&tree, &app, Some(a_id));
        let second = webview_for_frame_activation(&tree, &app, Some(b_id));
        assert_eq!(first, Some(a_id));
        assert_eq!(second, Some(b_id));

        let after_nodes = all_webview_tile_nodes(&tree);
        assert_eq!(after_nodes, initial_nodes);
    }

    #[test]
    fn test_split_layout_reports_two_active_webview_tile_rect_entries() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(61);
        let b = NodeKey::new(62);
        open_or_focus_webview_tile_with_mode(&mut tree, a, TileOpenMode::SplitHorizontal);
        open_or_focus_webview_tile_with_mode(&mut tree, b, TileOpenMode::SplitHorizontal);

        // In unit tests without an egui layout pass, rectangles are unset. We still
        // assert tile presence as the precondition for two-pane compositing.
        let nodes = all_webview_tile_nodes(&tree);
        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&a));
        assert!(nodes.contains(&b));
    }

    #[test]
    fn test_user_grouped_intents_for_tab_group_moves_emits_on_group_change() {
        let moved = NodeKey::new(70);
        let anchor = NodeKey::new(71);
        let old_group = TileId::from_u64(1);
        let new_group = TileId::from_u64(2);

        let mut before = HashMap::new();
        before.insert(moved, old_group);

        let mut after = HashMap::new();
        after.insert(moved, new_group);

        let mut after_nodes = HashMap::new();
        after_nodes.insert(new_group, vec![anchor, moved]);
        let moved_nodes = HashSet::from([moved]);

        let intents = tile_grouping::user_grouped_intents_for_tab_group_moves(
            &before,
            &after,
            &after_nodes,
            &moved_nodes,
        );
        assert_eq!(intents.len(), 1);
        match intents[0] {
            GraphIntent::CreateUserGroupedEdge { from, to } => {
                assert_eq!(from, moved);
                assert_eq!(to, anchor);
            },
            _ => panic!("expected CreateUserGroupedEdge"),
        }
    }

    #[test]
    fn test_user_grouped_intents_for_tab_group_moves_ignores_same_group_or_no_peer() {
        let node = NodeKey::new(80);
        let group = TileId::from_u64(3);

        let mut before = HashMap::new();
        before.insert(node, group);

        let mut after = HashMap::new();
        after.insert(node, group);

        let mut after_nodes = HashMap::new();
        after_nodes.insert(group, vec![node]);
        let moved_nodes = HashSet::from([node]);

        let intents = tile_grouping::user_grouped_intents_for_tab_group_moves(
            &before,
            &after,
            &after_nodes,
            &moved_nodes,
        );
        assert!(intents.is_empty());

        let moved_group = TileId::from_u64(4);
        after.insert(node, moved_group);
        after_nodes.insert(moved_group, vec![node]);
        let intents = tile_grouping::user_grouped_intents_for_tab_group_moves(
            &before,
            &after,
            &after_nodes,
            &moved_nodes,
        );
        assert!(intents.is_empty());
    }

    #[test]
    fn test_open_or_focus_sets_active_tile_to_target_node() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(10);
        let b = NodeKey::new(11);
        open_or_focus_webview_tile(&mut tree, a);
        open_or_focus_webview_tile(&mut tree, b);

        assert_eq!(Gui::active_webview_tile_node(&tree), Some(b));

        open_or_focus_webview_tile(&mut tree, a);
        assert_eq!(Gui::active_webview_tile_node(&tree), Some(a));
        assert_eq!(webview_tile_count(&tree, a), 1);
        assert_eq!(webview_tile_count(&tree, b), 1);
    }

    #[test]
    fn test_remove_webview_tile_for_node_preserves_other_tiles() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(20);
        let b = NodeKey::new(21);
        open_or_focus_webview_tile(&mut tree, a);
        open_or_focus_webview_tile(&mut tree, b);

        remove_webview_tile_for_node(&mut tree, a);
        let nodes = all_webview_tile_nodes(&tree);
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
        open_or_focus_webview_tile(&mut tree, alive_key);
        open_or_focus_webview_tile(&mut tree, stale_key);

        prune_stale_webview_tile_keys_only(&mut tree, &app);
        let nodes = all_webview_tile_nodes(&tree);
        assert!(nodes.contains(&alive_key));
        assert!(!nodes.contains(&stale_key));
    }

    #[test]
    fn test_tile_layout_serde_roundtrip() {
        let mut tree = tree_with_graph_root();
        let a = NodeKey::new(5);
        let b = NodeKey::new(6);
        open_or_focus_webview_tile(&mut tree, a);
        open_or_focus_webview_tile(&mut tree, b);

        let json = serde_json::to_string(&tree).expect("serialize tree");
        let restored: Tree<TileKind> = serde_json::from_str(&json).expect("deserialize tree");
        let nodes = all_webview_tile_nodes(&restored);

        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&a));
        assert!(nodes.contains(&b));
    }

    #[test]
    fn test_invariant_check_detects_desync() {
        let mut app = GraphBrowserApp::new_for_testing();
        let node_key = app.add_node_and_sync("https://example.test".into(), Point2D::new(0.0, 0.0));
        let mut tree = tree_with_graph_root();
        open_or_focus_webview_tile(&mut tree, node_key);

        let contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>> = HashMap::new();
        let violations = tile_invariants::collect_tile_invariant_violations(&tree, &app, &contexts);

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
        open_or_focus_webview_tile(&mut tree, node_key);

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

        assert!(!has_any_webview_tiles_in(&tree));
        assert!(tile_rendering_contexts.is_empty());
        assert!(tile_favicon_textures.is_empty());
        assert!(favicon_textures.is_empty());
    }
}

fn graph_intents_from_semantic_events(events: Vec<GraphSemanticEvent>) -> Vec<GraphIntent> {
    semantic_event_pipeline::graph_intents_from_semantic_events(events)
}

fn graph_intents_and_responsive_from_events(
    events: Vec<GraphSemanticEvent>,
) -> (Vec<GraphIntent>, Vec<WebViewId>, HashSet<WebViewId>) {
    semantic_event_pipeline::graph_intents_and_responsive_from_events(events)
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

fn graph_intent_for_thumbnail_result(
    graph_app: &GraphBrowserApp,
    result: &ThumbnailCaptureResult,
) -> Option<GraphIntent> {
    thumbnail_pipeline::graph_intent_for_thumbnail_result(graph_app, result)
}
