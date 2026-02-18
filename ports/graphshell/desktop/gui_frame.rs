/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};

use egui_tiles::Tree;
use euclid::Length;
use log::warn;
use servo::{DeviceIndependentPixel, OffscreenRenderingContext, WebViewId, WindowRenderingContext};
use winit::window::Window;

use super::dialog_panels::{self, DialogPanelsArgs};
use super::headed_window::HeadedWindow;
use super::lifecycle_reconcile::{self, RuntimeReconcileArgs};
use super::nav_targeting;
use super::semantic_event_pipeline;
use super::thumbnail_pipeline;
use super::thumbnail_pipeline::ThumbnailCaptureResult;
use super::tile_compositor;
use super::tile_invariants;
use super::tile_kind::TileKind;
use super::tile_render_pass::{self, TileRenderPassArgs};
use super::tile_runtime;
use super::toolbar_ui::{self, ToolbarUiArgs, ToolbarUiOutput};
use super::webview_backpressure::WebviewCreationBackpressureState;
use super::webview_controller;
use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::NodeKey;
use crate::input;
use crate::render;
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

pub(crate) struct PreFrameIngestArgs<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) graph_app: &'a GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) favicon_textures:
        &'a mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    pub(crate) thumbnail_capture_tx: &'a Sender<ThumbnailCaptureResult>,
    pub(crate) thumbnail_capture_rx: &'a Receiver<ThumbnailCaptureResult>,
    pub(crate) thumbnail_capture_in_flight: &'a mut HashSet<WebViewId>,
}

pub(crate) struct PreFrameIngestOutput {
    pub(crate) pending_open_child_webviews: Vec<WebViewId>,
    pub(crate) responsive_webviews: HashSet<WebViewId>,
}

pub(crate) fn ingest_pre_frame(
    args: PreFrameIngestArgs<'_>,
    frame_intents: &mut Vec<GraphIntent>,
) -> PreFrameIngestOutput {
    let PreFrameIngestArgs {
        ctx,
        graph_app,
        window,
        favicon_textures,
        thumbnail_capture_tx,
        thumbnail_capture_rx,
        thumbnail_capture_in_flight,
    } = args;

    frame_intents.extend(thumbnail_pipeline::load_pending_thumbnail_results(
        graph_app,
        window,
        thumbnail_capture_rx,
        thumbnail_capture_in_flight,
    ));
    let (semantic_intents, pending_open_child_webviews, responsive_webviews) =
        semantic_event_pipeline::graph_intents_and_responsive_from_events(
            window.take_pending_graph_events(),
        );
    frame_intents.extend(semantic_intents);
    frame_intents.extend(thumbnail_pipeline::load_pending_favicons(
        ctx,
        window,
        graph_app,
        favicon_textures,
    ));
    thumbnail_pipeline::request_pending_thumbnail_captures(
        graph_app,
        window,
        thumbnail_capture_tx,
        thumbnail_capture_in_flight,
    );

    PreFrameIngestOutput {
        pending_open_child_webviews,
        responsive_webviews,
    }
}

pub(crate) fn apply_intents_if_any(
    graph_app: &mut GraphBrowserApp,
    intents: &mut Vec<GraphIntent>,
) {
    if !intents.is_empty() {
        graph_app.apply_intents(std::mem::take(intents));
    }
}

pub(crate) fn open_pending_child_webviews_for_tiles<F>(
    graph_app: &GraphBrowserApp,
    pending_open_child_webviews: Vec<WebViewId>,
    mut open_for_node: F,
) where
    F: FnMut(NodeKey),
{
    for child_webview_id in pending_open_child_webviews {
        if let Some(node_key) = graph_app.get_node_for_webview(child_webview_id) {
            open_for_node(node_key);
        }
    }
}

pub(crate) struct KeyboardPhaseArgs<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub(crate) favicon_textures:
        &'a mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    pub(crate) app_state: &'a Option<Rc<RunningAppState>>,
    pub(crate) rendering_context: &'a Rc<OffscreenRenderingContext>,
    pub(crate) window_rendering_context: &'a Rc<WindowRenderingContext>,
    pub(crate) responsive_webviews: &'a HashSet<WebViewId>,
    pub(crate) webview_creation_backpressure:
        &'a mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    pub(crate) suppress_toggle_view: bool,
}

pub(crate) fn handle_keyboard_phase<F1, F2>(
    args: KeyboardPhaseArgs<'_>,
    frame_intents: &mut Vec<GraphIntent>,
    mut toggle_tile_view: F1,
    mut reset_runtime_webview_state: F2,
) where
    F1: FnMut(
        &mut Tree<TileKind>,
        &mut GraphBrowserApp,
        &ServoShellWindow,
        &Option<Rc<RunningAppState>>,
        &Rc<OffscreenRenderingContext>,
        &Rc<WindowRenderingContext>,
        &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        &HashSet<WebViewId>,
        &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
        &mut Vec<GraphIntent>,
    ),
    F2: FnMut(
        &mut Tree<TileKind>,
        &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
        &mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
        &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    ),
{
    let KeyboardPhaseArgs {
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
        responsive_webviews,
        webview_creation_backpressure,
        suppress_toggle_view,
    } = args;

    let mut keyboard_actions = input::collect_actions(ctx);
    if suppress_toggle_view {
        keyboard_actions.toggle_view = false;
    }
    if keyboard_actions.toggle_view {
        toggle_tile_view(
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
        reset_runtime_webview_state(
            tiles_tree,
            tile_rendering_contexts,
            tile_favicon_textures,
            favicon_textures,
        );
    }
    frame_intents.extend(input::intents_from_actions(&keyboard_actions));
}

fn active_webview_tile_node(tiles_tree: &Tree<TileKind>) -> Option<NodeKey> {
    tiles_tree
        .active_tiles()
        .into_iter()
        .find_map(|tile_id| match tiles_tree.tiles.get(tile_id) {
            Some(egui_tiles::Tile::Pane(TileKind::WebView(node_key))) => Some(*node_key),
            _ => None,
        })
}

pub(crate) struct ToolbarDialogPhaseArgs<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) winit_window: &'a Window,
    pub(crate) state: &'a RunningAppState,
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) focused_webview_hint: Option<WebViewId>,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) location: &'a mut String,
    pub(crate) location_dirty: &'a mut bool,
    pub(crate) location_submitted: &'a mut bool,
    pub(crate) focus_location_field_for_search: bool,
    pub(crate) show_data_dir_dialog: &'a mut bool,
    pub(crate) show_persistence_settings_dialog: &'a mut bool,
    pub(crate) show_clear_data_confirm: &'a mut bool,
    pub(crate) data_dir_input: &'a mut String,
    pub(crate) data_dir_status: &'a mut Option<String>,
    pub(crate) snapshot_interval_input: &'a mut String,
    pub(crate) persistence_settings_status: &'a mut Option<String>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub(crate) favicon_textures:
        &'a mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
}

pub(crate) struct ToolbarDialogPhaseOutput {
    pub(crate) is_graph_view: bool,
    pub(crate) toolbar_output: ToolbarUiOutput,
}

pub(crate) fn handle_toolbar_dialog_phase(
    args: ToolbarDialogPhaseArgs<'_>,
    frame_intents: &mut Vec<GraphIntent>,
) -> ToolbarDialogPhaseOutput {
    let ToolbarDialogPhaseArgs {
        ctx,
        winit_window,
        state,
        graph_app,
        window,
        tiles_tree,
        focused_webview_hint,
        can_go_back,
        can_go_forward,
        location,
        location_dirty,
        location_submitted,
        focus_location_field_for_search,
        show_data_dir_dialog,
        show_persistence_settings_dialog,
        show_clear_data_confirm,
        data_dir_input,
        data_dir_status,
        snapshot_interval_input,
        persistence_settings_status,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
    } = args;

    let active_webview_node = active_webview_tile_node(tiles_tree);
    let focused_toolbar_webview =
        tile_compositor::focused_webview_id_for_tree(tiles_tree, graph_app, focused_webview_hint);
    let focused_toolbar_node = nav_targeting::focused_toolbar_node(
        graph_app,
        active_webview_node,
        focused_toolbar_webview,
        graph_app.get_single_selected_node(),
    );
    let has_webview_tiles = tile_runtime::has_any_webview_tiles(tiles_tree);
    let is_graph_view = !has_webview_tiles;

    let toolbar_output = toolbar_ui::render_toolbar_ui(ToolbarUiArgs {
        ctx,
        winit_window,
        state,
        graph_app,
        window,
        focused_toolbar_node,
        has_webview_tiles,
        can_go_back,
        can_go_forward,
        location,
        location_dirty,
        location_submitted,
        focus_location_field_for_search,
        show_data_dir_dialog,
        show_persistence_settings_dialog,
        show_clear_data_confirm,
        frame_intents,
    });

    dialog_panels::render_dialog_panels(DialogPanelsArgs {
        ctx,
        graph_app,
        window,
        tiles_tree,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
        frame_intents,
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
    });

    ToolbarDialogPhaseOutput {
        is_graph_view,
        toolbar_output,
    }
}

pub(crate) struct LifecycleReconcilePhaseArgs<'a> {
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) app_state: &'a Option<Rc<RunningAppState>>,
    pub(crate) rendering_context: &'a Rc<OffscreenRenderingContext>,
    pub(crate) window_rendering_context: &'a Rc<WindowRenderingContext>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub(crate) favicon_textures:
        &'a mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    pub(crate) responsive_webviews: &'a HashSet<WebViewId>,
    pub(crate) webview_creation_backpressure:
        &'a mut HashMap<NodeKey, WebviewCreationBackpressureState>,
}

pub(crate) fn run_lifecycle_reconcile_and_apply(
    args: LifecycleReconcilePhaseArgs<'_>,
    frame_intents: &mut Vec<GraphIntent>,
) {
    let LifecycleReconcilePhaseArgs {
        graph_app,
        tiles_tree,
        window,
        app_state,
        rendering_context,
        window_rendering_context,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
        responsive_webviews,
        webview_creation_backpressure,
    } = args;

    lifecycle_reconcile::reconcile_runtime(RuntimeReconcileArgs {
        graph_app,
        tiles_tree,
        window,
        app_state,
        rendering_context,
        window_rendering_context,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
        responsive_webviews,
        webview_creation_backpressure,
        frame_intents,
    });

    apply_intents_if_any(graph_app, frame_intents);
}

pub(crate) struct PostRenderPhaseArgs<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) headed_window: &'a HeadedWindow,
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub(crate) toolbar_height: &'a mut Length<f32, DeviceIndependentPixel>,
    pub(crate) graph_search_matches: &'a [NodeKey],
    pub(crate) graph_search_active_match_index: Option<usize>,
    pub(crate) graph_search_filter_mode: bool,
    pub(crate) search_query_active: bool,
    pub(crate) app_state: &'a Option<Rc<RunningAppState>>,
    pub(crate) rendering_context: &'a Rc<OffscreenRenderingContext>,
    pub(crate) window_rendering_context: &'a Rc<WindowRenderingContext>,
    pub(crate) responsive_webviews: &'a HashSet<WebViewId>,
    pub(crate) webview_creation_backpressure:
        &'a mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    pub(crate) focused_webview_hint: &'a mut Option<WebViewId>,
}

pub(crate) fn run_post_render_phase<FActive>(
    args: PostRenderPhaseArgs<'_>,
    active_graph_search_match: FActive,
) where
    FActive: Fn(&[NodeKey], Option<usize>) -> Option<NodeKey>,
{
    let PostRenderPhaseArgs {
        ctx,
        graph_app,
        window,
        headed_window,
        tiles_tree,
        tile_rendering_contexts,
        tile_favicon_textures,
        toolbar_height,
        graph_search_matches,
        graph_search_active_match_index,
        graph_search_filter_mode,
        search_query_active,
        app_state,
        rendering_context,
        window_rendering_context,
        responsive_webviews,
        webview_creation_backpressure,
        focused_webview_hint,
    } = args;

    #[cfg(debug_assertions)]
    {
        for violation in tile_invariants::collect_tile_invariant_violations(
            tiles_tree,
            graph_app,
            tile_rendering_contexts,
        ) {
            warn!("{violation}");
        }
    }

    let has_webview_tiles = tile_runtime::has_any_webview_tiles(tiles_tree);
    let is_graph_view = !has_webview_tiles;

    *toolbar_height = Length::new(ctx.available_rect().min.y);
    graph_app.check_periodic_snapshot();

    let focused_dialog_webview =
        tile_compositor::focused_webview_id_for_tree(tiles_tree, graph_app, *focused_webview_hint);
    headed_window.for_each_active_dialog(
        window,
        focused_dialog_webview,
        *toolbar_height,
        |dialog| dialog.update(ctx),
    );

    let mut post_render_intents = Vec::new();
    if is_graph_view || has_webview_tiles {
        let search_matches: HashSet<NodeKey> = graph_search_matches.iter().copied().collect();
        let active_search_match =
            active_graph_search_match(graph_search_matches, graph_search_active_match_index);
        post_render_intents.extend(tile_render_pass::run_tile_render_pass(TileRenderPassArgs {
            ctx,
            graph_app,
            window,
            tiles_tree,
            tile_rendering_contexts,
            tile_favicon_textures,
            graph_search_matches: &search_matches,
            active_search_match,
            graph_search_filter_mode,
            search_query_active,
            app_state,
            rendering_context,
            window_rendering_context,
            responsive_webviews,
            webview_creation_backpressure,
            focused_webview_hint,
        }));
    }
    if !post_render_intents.is_empty() {
        graph_app.apply_intents(post_render_intents);
    }

    render::render_physics_panel(ctx, graph_app);
    render::render_help_panel(ctx, graph_app);
}
