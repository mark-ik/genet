/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use egui_tiles::{Container, Tile, TileId, Tiles, Tree};
use euclid::Length;
use log::warn;
use servo::{DeviceIndependentPixel, OffscreenRenderingContext, WebViewId, WindowRenderingContext};
use winit::window::Window;

use super::dialog_panels::{self, DialogPanelsArgs};
use super::headed_window::HeadedWindow;
use super::lifecycle_reconcile::{self, RuntimeReconcileArgs};
use super::nav_targeting;
use super::persistence_ops;
use super::semantic_event_pipeline;
use super::thumbnail_pipeline;
use super::thumbnail_pipeline::ThumbnailCaptureResult;
use super::tile_compositor;
use super::tile_invariants;
use super::tile_kind::TileKind;
use super::tile_render_pass::{self, TileRenderPassArgs};
use super::tile_runtime;
use super::tile_view_ops;
use super::toolbar_ui::{self, OmnibarSearchSession, ToolbarUiArgs, ToolbarUiOutput};
use super::webview_backpressure::WebviewCreationBackpressureState;
use super::webview_controller;
use crate::app::{
    GraphBrowserApp, GraphIntent, PendingConnectedOpenScope, PendingTileOpenMode,
    UnsavedWorkspacePromptAction, UnsavedWorkspacePromptRequest,
};
use crate::graph::NodeKey;
use crate::input;
use crate::render;
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

fn tile_open_mode_from_pending(
    mode: crate::app::PendingTileOpenMode,
) -> tile_view_ops::TileOpenMode {
    match mode {
        crate::app::PendingTileOpenMode::Tab => tile_view_ops::TileOpenMode::Tab,
        crate::app::PendingTileOpenMode::SplitHorizontal => {
            tile_view_ops::TileOpenMode::SplitHorizontal
        },
    }
}

const MAX_CONNECTED_SPLIT_PANES: usize = 4;
const MAX_CONNECTED_OPEN_NODES: usize = 12;

fn find_webview_tile_id(tree: &Tree<TileKind>, node_key: NodeKey) -> Option<TileId> {
    tree.tiles.iter().find_map(|(tile_id, tile)| match tile {
        Tile::Pane(TileKind::WebView(key)) if *key == node_key => Some(*tile_id),
        _ => None,
    })
}

fn ensure_webview_tile_id(tree: &mut Tree<TileKind>, node_key: NodeKey) -> TileId {
    if let Some(tile_id) = find_webview_tile_id(tree, node_key) {
        if let Some(parent_id) = tree.tiles.parent_of(tile_id)
            && matches!(
                tree.tiles.get(parent_id),
                Some(Tile::Container(Container::Tabs(_)))
            )
        {
            return parent_id;
        }
        return tree.tiles.insert_tab_tile(vec![tile_id]);
    }
    let pane_id = tree.tiles.insert_pane(TileKind::WebView(node_key));
    tree.tiles.insert_tab_tile(vec![pane_id])
}

fn workspace_nodes_from_tree(tree: &Tree<TileKind>) -> Vec<NodeKey> {
    tree.tiles
        .iter()
        .filter_map(|(_, tile)| match tile {
            Tile::Pane(TileKind::WebView(key)) => Some(*key),
            _ => None,
        })
        .collect()
}

fn restore_named_workspace_snapshot(
    graph_app: &mut GraphBrowserApp,
    tiles_tree: &mut Tree<TileKind>,
    name: &str,
    mut routed_focus_node: Option<NodeKey>,
) {
    if let Some(layout_json) = graph_app.load_workspace_layout_json(name) {
        match serde_json::from_str::<Tree<TileKind>>(&layout_json) {
            Ok(mut restored_tree) => {
                if let Ok(current_layout_json) = serde_json::to_string(tiles_tree) {
                    graph_app.capture_undo_checkpoint(Some(current_layout_json));
                }
                tile_runtime::prune_stale_webview_tile_keys_only(&mut restored_tree, graph_app);
                if restored_tree.root().is_some() {
                    if let Some(node_key) = routed_focus_node.take()
                        && graph_app.graph.get_node(node_key).is_some()
                    {
                        tile_view_ops::open_or_focus_webview_tile_with_mode(
                            &mut restored_tree,
                            node_key,
                            tile_view_ops::TileOpenMode::Tab,
                        );
                        graph_app
                            .apply_intents([GraphIntent::PromoteNodeToActive { key: node_key }]);
                    }
                    graph_app
                        .note_workspace_activated(name, workspace_nodes_from_tree(&restored_tree));
                    graph_app.mark_session_workspace_layout_json(&layout_json);
                    *tiles_tree = restored_tree;
                } else if let Some(node_key) = routed_focus_node.take() {
                    warn!(
                        "Workspace snapshot '{name}' is empty after stale-key prune; falling back to current workspace open"
                    );
                    graph_app.select_node(node_key, false);
                    graph_app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
                }
            },
            Err(e) => {
                warn!("Failed to deserialize workspace snapshot '{name}': {e}");
                if let Some(node_key) = routed_focus_node.take() {
                    graph_app.select_node(node_key, false);
                    graph_app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
                }
            },
        }
    } else if let Some(node_key) = routed_focus_node.take() {
        warn!("Workspace snapshot '{name}' not found; falling back to current workspace open");
        graph_app.select_node(node_key, false);
        graph_app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
    }
}

fn workspace_tree_with_single_node(node_key: NodeKey) -> Tree<TileKind> {
    let mut tiles = Tiles::default();
    let pane_id = tiles.insert_pane(TileKind::WebView(node_key));
    let root = tiles.insert_tab_tile(vec![pane_id]);
    Tree::new("graphshell_workspace_layout", root, tiles)
}

fn add_node_to_named_workspace_snapshot(
    graph_app: &mut GraphBrowserApp,
    name: &str,
    node_key: NodeKey,
) {
    if graph_app.graph.get_node(node_key).is_none() {
        warn!("Cannot add missing node {node_key:?} to workspace '{name}'");
        return;
    }
    if GraphBrowserApp::is_reserved_workspace_layout_name(name) {
        warn!("Cannot add node to reserved workspace '{name}'");
        return;
    }

    let mut workspace_tree = graph_app
        .load_workspace_layout_json(name)
        .and_then(|layout| serde_json::from_str::<Tree<TileKind>>(&layout).ok())
        .unwrap_or_else(|| workspace_tree_with_single_node(node_key));
    tile_runtime::prune_stale_webview_tile_keys_only(&mut workspace_tree, graph_app);
    if workspace_tree.root().is_none() {
        workspace_tree = workspace_tree_with_single_node(node_key);
    }

    tile_view_ops::open_or_focus_webview_tile_with_mode(
        &mut workspace_tree,
        node_key,
        tile_view_ops::TileOpenMode::Tab,
    );
    match serde_json::to_string(&workspace_tree) {
        Ok(layout_json) => {
            graph_app.save_workspace_layout_json(name, &layout_json);
            let membership_index = persistence_ops::build_membership_index_from_layouts(graph_app);
            graph_app.init_membership_index(membership_index);
        },
        Err(e) => {
            warn!("Failed to serialize workspace snapshot '{name}' after add-tab operation: {e}")
        },
    }
}

fn apply_connected_split_layout(tree: &mut Tree<TileKind>, nodes: &[NodeKey]) {
    if nodes.is_empty() {
        return;
    }
    let split_count = nodes.len().min(MAX_CONNECTED_SPLIT_PANES);
    let split_tile_ids: Vec<TileId> = nodes
        .iter()
        .take(split_count)
        .copied()
        .map(|key| ensure_webview_tile_id(tree, key))
        .collect();
    let overflow_tile_ids: Vec<TileId> = nodes
        .iter()
        .skip(split_count)
        .copied()
        .map(|key| ensure_webview_tile_id(tree, key))
        .collect();

    let row1 = match split_tile_ids.as_slice() {
        [a] => *a,
        [a, b, ..] => tree.tiles.insert_horizontal_tile(vec![*a, *b]),
        [] => return,
    };

    let grid_root = if split_tile_ids.len() > 2 {
        let row2 = match split_tile_ids.as_slice() {
            [_, _, c] => *c,
            [_, _, c, d, ..] => tree.tiles.insert_horizontal_tile(vec![*c, *d]),
            _ => return,
        };
        tree.tiles.insert_vertical_tile(vec![row1, row2])
    } else {
        row1
    };

    tree.root = if overflow_tile_ids.is_empty() {
        Some(grid_root)
    } else {
        let overflow_tabs = tree.tiles.insert_tab_tile(overflow_tile_ids);
        Some(
            tree.tiles
                .insert_vertical_tile(vec![grid_root, overflow_tabs]),
        )
    };
}

fn undirected_neighbors_sorted(graph_app: &GraphBrowserApp, node_key: NodeKey) -> Vec<NodeKey> {
    let mut neighbors: Vec<NodeKey> = graph_app
        .graph
        .out_neighbors(node_key)
        .chain(graph_app.graph.in_neighbors(node_key))
        .filter(|key| *key != node_key && graph_app.graph.get_node(*key).is_some())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    neighbors.sort_by_key(|key| key.index());
    neighbors
}

fn connected_candidates_with_depth(
    graph_app: &GraphBrowserApp,
    source: NodeKey,
    scope: PendingConnectedOpenScope,
) -> Vec<(NodeKey, u8)> {
    match scope {
        PendingConnectedOpenScope::Neighbors => undirected_neighbors_sorted(graph_app, source)
            .into_iter()
            .map(|key| (key, 1))
            .collect(),
        PendingConnectedOpenScope::Connected => {
            let mut out = Vec::new();
            let mut visited = HashSet::from([source]);
            let mut queue = VecDeque::from([(source, 0_u8)]);

            while let Some((current, depth)) = queue.pop_front() {
                if depth >= 2 {
                    continue;
                }
                for neighbor in undirected_neighbors_sorted(graph_app, current) {
                    if !visited.insert(neighbor) {
                        continue;
                    }
                    let next_depth = depth + 1;
                    out.push((neighbor, next_depth));
                    if next_depth < 2 {
                        queue.push_back((neighbor, next_depth));
                    }
                }
            }

            out
        },
    }
}

fn connected_targets_for_open(
    graph_app: &GraphBrowserApp,
    source: NodeKey,
    scope: PendingConnectedOpenScope,
) -> Vec<NodeKey> {
    let mut candidates = connected_candidates_with_depth(graph_app, source, scope);
    let cap = MAX_CONNECTED_OPEN_NODES.saturating_sub(1);

    if candidates.len() > cap {
        candidates.sort_by(|(a, depth_a), (b, depth_b)| {
            graph_app
                .workspace_recency_seq_for_node(*b)
                .cmp(&graph_app.workspace_recency_seq_for_node(*a))
                .then_with(|| depth_a.cmp(depth_b))
                .then_with(|| a.index().cmp(&b.index()))
        });
        candidates.truncate(cap);
    }

    candidates.sort_by(|(a, depth_a), (b, depth_b)| {
        depth_a
            .cmp(depth_b)
            .then_with(|| {
                graph_app
                    .workspace_recency_seq_for_node(*b)
                    .cmp(&graph_app.workspace_recency_seq_for_node(*a))
            })
            .then_with(|| a.index().cmp(&b.index()))
    });
    candidates.into_iter().map(|(key, _)| key).collect()
}

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
    tiles_tree: &Tree<TileKind>,
    intents: &mut Vec<GraphIntent>,
) {
    if intents.is_empty() {
        return;
    }

    let mut undo_count = 0usize;
    let mut redo_count = 0usize;
    let mut apply_list = Vec::new();
    for intent in std::mem::take(intents) {
        match intent {
            GraphIntent::Undo => undo_count += 1,
            GraphIntent::Redo => redo_count += 1,
            other => apply_list.push(other),
        }
    }

    let layout_json = serde_json::to_string(tiles_tree).ok();
    if !apply_list.is_empty() {
        if apply_list.iter().any(is_user_undoable_intent) {
            graph_app.capture_undo_checkpoint(layout_json.clone());
        }
        graph_app.apply_intents(apply_list);
    }

    for _ in 0..undo_count {
        let _ = graph_app.perform_undo(layout_json.clone());
    }
    for _ in 0..redo_count {
        let _ = graph_app.perform_redo(layout_json.clone());
    }
}

fn is_user_undoable_intent(intent: &GraphIntent) -> bool {
    matches!(
        intent,
        GraphIntent::CreateNodeNearCenter
            | GraphIntent::CreateNodeAtUrl { .. }
            | GraphIntent::RemoveSelectedNodes
            | GraphIntent::ClearGraph
            | GraphIntent::SetNodePosition { .. }
            | GraphIntent::SetNodeUrl { .. }
            | GraphIntent::CreateUserGroupedEdge { .. }
            | GraphIntent::RemoveEdge { .. }
            | GraphIntent::ExecuteEdgeCommand { .. }
            | GraphIntent::SetNodePinned { .. }
            | GraphIntent::TogglePrimaryNodePin
            | GraphIntent::PromoteNodeToActive { .. }
            | GraphIntent::DemoteNodeToWarm { .. }
            | GraphIntent::DemoteNodeToCold { .. }
    )
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

pub(crate) fn active_webview_tile_node(tiles_tree: &Tree<TileKind>) -> Option<NodeKey> {
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
    pub(crate) graph_surface_focused: bool,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) location: &'a mut String,
    pub(crate) location_dirty: &'a mut bool,
    pub(crate) location_submitted: &'a mut bool,
    pub(crate) focus_location_field_for_search: bool,
    pub(crate) show_data_dir_dialog: &'a mut bool,
    pub(crate) show_persistence_settings_dialog: &'a mut bool,
    pub(crate) show_clear_data_confirm: &'a mut bool,
    pub(crate) omnibar_search_session: &'a mut Option<OmnibarSearchSession>,
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
        graph_surface_focused,
        can_go_back,
        can_go_forward,
        location,
        location_dirty,
        location_submitted,
        focus_location_field_for_search,
        show_data_dir_dialog,
        show_persistence_settings_dialog,
        show_clear_data_confirm,
        omnibar_search_session,
        data_dir_input,
        data_dir_status,
        snapshot_interval_input,
        persistence_settings_status,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
    } = args;

    let active_webview_node = active_webview_tile_node(tiles_tree);
    let focused_toolbar_webview = if graph_surface_focused {
        None
    } else {
        tile_compositor::focused_webview_id_for_tree(tiles_tree, graph_app, focused_webview_hint)
    };
    let focused_toolbar_node = nav_targeting::focused_toolbar_node(
        graph_app,
        active_webview_node,
        focused_toolbar_webview,
        graph_app.get_single_selected_node(),
    );
    let has_webview_tiles = tile_runtime::has_any_webview_tiles(tiles_tree);
    let is_graph_view = !has_webview_tiles;
    if !is_graph_view {
        graph_app.hovered_graph_node = None;
    }

    let toolbar_output = toolbar_ui::render_toolbar_ui(ToolbarUiArgs {
        ctx,
        winit_window,
        state,
        graph_app,
        window,
        tiles_tree,
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
        omnibar_search_session,
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

    apply_intents_if_any(graph_app, tiles_tree, frame_intents);
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
    pub(crate) graph_surface_focused: bool,
    pub(crate) focus_ring_webview_id: &'a mut Option<WebViewId>,
    pub(crate) focus_ring_started_at: &'a mut Option<Instant>,
    pub(crate) focus_ring_duration: Duration,
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
        graph_surface_focused,
        focus_ring_webview_id,
        focus_ring_started_at,
        focus_ring_duration,
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

    let focused_dialog_webview = if graph_surface_focused {
        None
    } else {
        tile_compositor::focused_webview_id_for_tree(tiles_tree, graph_app, *focused_webview_hint)
    };
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
            graph_surface_focused,
            focus_ring_webview_id,
            focus_ring_started_at,
            focus_ring_duration,
        }));
    }
    apply_intents_if_any(graph_app, tiles_tree, &mut post_render_intents);

    render::render_physics_panel(ctx, graph_app);
    render::render_help_panel(ctx, graph_app);
    let focused_pane_node = focused_dialog_webview
        .and_then(|webview_id| graph_app.get_node_for_webview(webview_id))
        .or_else(|| active_webview_tile_node(tiles_tree));
    render::render_command_palette_panel(
        ctx,
        graph_app,
        graph_app.hovered_graph_node,
        focused_pane_node,
    );
    render::render_radial_command_menu(
        ctx,
        graph_app,
        graph_app.hovered_graph_node,
        focused_pane_node,
    );
    let persistence_panel_layout_json = serde_json::to_string(tiles_tree).ok();
    render::render_persistence_panel(
        ctx,
        graph_app,
        focused_pane_node,
        persistence_panel_layout_json.as_deref(),
    );
    render::render_choose_workspace_picker(ctx, graph_app);
    render::render_unsaved_workspace_prompt(ctx, graph_app);

    if let Some((request, action)) = graph_app.take_unsaved_workspace_prompt_resolution() {
        match (request, action) {
            (
                UnsavedWorkspacePromptRequest::WorkspaceSwitch { name, focus_node },
                UnsavedWorkspacePromptAction::ProceedWithoutSaving,
            ) => {
                restore_named_workspace_snapshot(graph_app, tiles_tree, &name, focus_node);
            },
            (
                UnsavedWorkspacePromptRequest::WorkspaceSwitch { .. },
                UnsavedWorkspacePromptAction::Cancel,
            ) => {},
        }
    }

    if graph_app.take_pending_save_workspace_snapshot() {
        match serde_json::to_string(tiles_tree) {
            Ok(layout_json) => graph_app.save_tile_layout_json(&layout_json),
            Err(e) => warn!("Failed to serialize tile layout for workspace snapshot: {e}"),
        }
    }

    if let Some(name) = graph_app.take_pending_save_workspace_snapshot_named() {
        match serde_json::to_string(tiles_tree) {
            Ok(layout_json) => {
                graph_app.save_workspace_layout_json(&name, &layout_json);
                let membership_index =
                    persistence_ops::build_membership_index_from_layouts(graph_app);
                graph_app.init_membership_index(membership_index);
            },
            Err(e) => warn!("Failed to serialize tile layout for workspace snapshot '{name}': {e}"),
        }
    }

    if graph_app.take_pending_prune_empty_workspaces() {
        let deleted = persistence_ops::prune_empty_named_workspaces(graph_app);
        warn!("Pruned {deleted} empty named workspaces");
    }

    if let Some(keep) = graph_app.take_pending_keep_latest_named_workspaces() {
        let deleted = persistence_ops::keep_latest_named_workspaces(graph_app, keep);
        warn!("Removed {deleted} named workspaces beyond latest {keep}");
    }

    if let Some(name) = graph_app.take_pending_restore_workspace_snapshot_named() {
        let focus_node = graph_app.take_pending_workspace_restore_focus_node();
        if graph_app.should_prompt_unsaved_workspace_save() {
            if graph_app.consume_unsaved_workspace_prompt_warning() {
                warn!("Current workspace has unsaved graph changes before switching to '{name}'");
            }
            graph_app.request_unsaved_workspace_prompt(
                UnsavedWorkspacePromptRequest::WorkspaceSwitch { name, focus_node },
            );
        } else {
            restore_named_workspace_snapshot(graph_app, tiles_tree, &name, focus_node);
        }
    }

    if let Some((node_key, workspace_name)) = graph_app.take_pending_add_node_to_workspace() {
        add_node_to_named_workspace_snapshot(graph_app, &workspace_name, node_key);
    }

    if let Some(name) = graph_app.take_pending_save_graph_snapshot_named()
        && let Err(e) = graph_app.save_named_graph_snapshot(&name)
    {
        warn!("Failed to save named graph snapshot '{name}': {e}");
    }

    if let Some(name) = graph_app.take_pending_delete_graph_snapshot_named()
        && let Err(e) = graph_app.delete_named_graph_snapshot(&name)
    {
        warn!("Failed to delete named graph snapshot '{name}': {e}");
    }

    if let Some(name) = graph_app.take_pending_restore_graph_snapshot_named() {
        if let Ok(layout_json) = serde_json::to_string(tiles_tree) {
            graph_app.capture_undo_checkpoint(Some(layout_json));
        }
        let close_intents = webview_controller::close_all_webviews(graph_app, window);
        if !close_intents.is_empty() {
            graph_app.apply_intents(close_intents);
        }
        match graph_app.load_named_graph_snapshot(&name) {
            Ok(()) => {
                tile_rendering_contexts.clear();
                tile_favicon_textures.clear();
                webview_creation_backpressure.clear();
                *focused_webview_hint = None;
                let mut tiles = Tiles::default();
                let graph_tile_id = tiles.insert_pane(TileKind::Graph);
                *tiles_tree = Tree::new("graphshell_tiles", graph_tile_id, tiles);
            },
            Err(e) => warn!("Failed to load named graph snapshot '{name}': {e}"),
        }
    }

    if graph_app.take_pending_restore_graph_snapshot_latest() {
        if let Ok(layout_json) = serde_json::to_string(tiles_tree) {
            graph_app.capture_undo_checkpoint(Some(layout_json));
        }
        let close_intents = webview_controller::close_all_webviews(graph_app, window);
        if !close_intents.is_empty() {
            graph_app.apply_intents(close_intents);
        }
        match graph_app.load_latest_graph_snapshot() {
            Ok(()) => {
                tile_rendering_contexts.clear();
                tile_favicon_textures.clear();
                webview_creation_backpressure.clear();
                *focused_webview_hint = None;
                let mut tiles = Tiles::default();
                let graph_tile_id = tiles.insert_pane(TileKind::Graph);
                *tiles_tree = Tree::new("graphshell_tiles", graph_tile_id, tiles);
            },
            Err(e) => warn!("Failed to load autosaved latest graph snapshot: {e}"),
        }
    }

    if let Some(node_key) = graph_app.take_pending_detach_node_to_split() {
        if let Ok(layout_json) = serde_json::to_string(tiles_tree) {
            graph_app.capture_undo_checkpoint(Some(layout_json));
        }
        tile_view_ops::detach_webview_tile_to_split(tiles_tree, node_key);
    }

    if let Some((source, open_mode, scope)) = graph_app.take_pending_open_connected_from()
        && graph_app.graph.get_node(source).is_some()
    {
        if let Ok(layout_json) = serde_json::to_string(tiles_tree) {
            graph_app.capture_undo_checkpoint(Some(layout_json));
        }
        let connected = connected_targets_for_open(graph_app, source, scope);

        let mut intents = Vec::with_capacity(connected.len() + 2);
        intents.push(GraphIntent::SelectNode {
            key: source,
            multi_select: false,
        });
        intents.push(GraphIntent::PromoteNodeToActive { key: source });
        for node in &connected {
            intents.push(GraphIntent::PromoteNodeToActive { key: *node });
        }
        graph_app.apply_intents(intents);

        let mut ordered = Vec::with_capacity(connected.len() + 1);
        ordered.push(source);
        ordered.extend(connected);

        graph_app.mark_current_workspace_synthesized();
        let tile_mode = tile_open_mode_from_pending(open_mode);
        match tile_mode {
            tile_view_ops::TileOpenMode::Tab => {
                for node in ordered {
                    tile_view_ops::open_or_focus_webview_tile_with_mode(
                        tiles_tree,
                        node,
                        tile_view_ops::TileOpenMode::Tab,
                    );
                }
            },
            tile_view_ops::TileOpenMode::SplitHorizontal => {
                // One-shot connected-open uses a compact 2-up / 2x2 split policy (max 4),
                // with overflow grouped into tabs below the split area.
                apply_connected_split_layout(tiles_tree, &ordered);
            },
        }
    }

    if let Some(layout_json) = graph_app.take_pending_history_workspace_layout_json() {
        match serde_json::from_str::<Tree<TileKind>>(&layout_json) {
            Ok(mut restored_tree) => {
                tile_runtime::prune_stale_webview_tile_keys_only(&mut restored_tree, graph_app);
                if restored_tree.root().is_some() {
                    *tiles_tree = restored_tree;
                    graph_app.mark_session_workspace_layout_json(&layout_json);
                }
            },
            Err(e) => warn!("Failed to deserialize undo/redo workspace snapshot: {e}"),
        }
    }

    let prompt_pending = graph_app.unsaved_workspace_prompt_request().is_some();
    // Session autosave should not block on unsaved-workspace prompts. Prompting
    // is reserved for explicit workspace-switch actions.
    if !prompt_pending {
        match serde_json::to_string(tiles_tree) {
            Ok(layout_json) => {
                graph_app.save_session_workspace_layout_json_if_changed(&layout_json)
            },
            Err(e) => warn!("Failed to serialize session workspace layout: {e}"),
        }
    }
}
