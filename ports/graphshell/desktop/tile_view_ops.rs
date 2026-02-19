/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use egui_tiles::{Container, Tile, Tree};
use servo::{OffscreenRenderingContext, WebViewId, WindowRenderingContext};

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::desktop::webview_backpressure::{self, WebviewCreationBackpressureState};
use crate::graph::NodeKey;
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileOpenMode {
    Tab,
    SplitHorizontal,
}

pub(crate) struct ToggleTileViewArgs<'a> {
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) app_state: &'a Option<Rc<RunningAppState>>,
    pub(crate) base_rendering_context: &'a Rc<OffscreenRenderingContext>,
    pub(crate) window_rendering_context: &'a Rc<WindowRenderingContext>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) responsive_webviews: &'a HashSet<WebViewId>,
    pub(crate) webview_creation_backpressure:
        &'a mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    pub(crate) lifecycle_intents: &'a mut Vec<GraphIntent>,
}

pub(crate) fn preferred_detail_node(graph_app: &GraphBrowserApp) -> Option<NodeKey> {
    graph_app
        .get_single_selected_node()
        .or_else(|| graph_app.graph.nodes().next().map(|(key, _)| key))
}

pub(crate) fn open_or_focus_webview_tile(tiles_tree: &mut Tree<TileKind>, node_key: NodeKey) {
    open_or_focus_webview_tile_with_mode(tiles_tree, node_key, TileOpenMode::Tab);
}

pub(crate) fn open_or_focus_webview_tile_with_mode(
    tiles_tree: &mut Tree<TileKind>,
    node_key: NodeKey,
    mode: TileOpenMode,
) {
    if tiles_tree.make_active(
        |_, tile| matches!(tile, Tile::Pane(TileKind::WebView(key)) if *key == node_key),
    ) {
        return;
    }

    let webview_tile_id = tiles_tree.tiles.insert_pane(TileKind::WebView(node_key));
    let split_leaf_tile_id = tiles_tree.tiles.insert_tab_tile(vec![webview_tile_id]);
    let Some(root_id) = tiles_tree.root() else {
        tiles_tree.root = Some(match mode {
            TileOpenMode::Tab => webview_tile_id,
            TileOpenMode::SplitHorizontal => split_leaf_tile_id,
        });
        return;
    };

    match mode {
        TileOpenMode::Tab => {
            if let Some(Tile::Container(Container::Tabs(tabs))) = tiles_tree.tiles.get_mut(root_id)
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
        TileOpenMode::SplitHorizontal => {
            // Never split directly against a raw leaf pane: wrap it in tabs first.
            let split_lhs_id = if matches!(
                tiles_tree.tiles.get(root_id),
                Some(Tile::Pane(TileKind::WebView(_)))
            ) {
                let wrapped = tiles_tree.tiles.insert_tab_tile(vec![root_id]);
                tiles_tree.root = Some(wrapped);
                wrapped
            } else {
                root_id
            };

            if let Some(Tile::Container(Container::Linear(linear))) =
                tiles_tree.tiles.get_mut(split_lhs_id)
            {
                linear.add_child(split_leaf_tile_id);
                return;
            }
            let split_root = tiles_tree
                .tiles
                .insert_horizontal_tile(vec![split_lhs_id, split_leaf_tile_id]);
            tiles_tree.root = Some(split_root);
        },
    }
}

pub(crate) fn detach_webview_tile_to_split(tiles_tree: &mut Tree<TileKind>, node_key: NodeKey) {
    let existing_tile_id = tiles_tree
        .tiles
        .iter()
        .find_map(|(tile_id, tile)| match tile {
            Tile::Pane(TileKind::WebView(key)) if *key == node_key => Some(*tile_id),
            _ => None,
        });

    if let Some(tile_id) = existing_tile_id {
        tiles_tree.remove_recursively(tile_id);
    }
    open_or_focus_webview_tile_with_mode(tiles_tree, node_key, TileOpenMode::SplitHorizontal);
}

pub(crate) fn toggle_tile_view(args: ToggleTileViewArgs<'_>) {
    if tile_runtime::has_any_webview_tiles(args.tiles_tree) {
        let webview_nodes = tile_runtime::all_webview_tile_nodes(args.tiles_tree);
        let tile_ids: Vec<_> = args
            .tiles_tree
            .tiles
            .iter()
            .filter_map(|(tile_id, tile)| match tile {
                Tile::Pane(TileKind::WebView(_)) => Some(*tile_id),
                _ => None,
            })
            .collect();
        for tile_id in tile_ids {
            args.tiles_tree.remove_recursively(tile_id);
        }
        for node_key in webview_nodes {
            tile_runtime::close_webview_for_node(
                args.graph_app,
                args.window,
                args.tile_rendering_contexts,
                node_key,
                args.lifecycle_intents,
            );
        }
    } else if let Some(node_key) = preferred_detail_node(args.graph_app) {
        open_or_focus_webview_tile(args.tiles_tree, node_key);
        webview_backpressure::ensure_webview_for_node(
            args.graph_app,
            args.window,
            args.app_state,
            args.base_rendering_context,
            args.window_rendering_context,
            args.tile_rendering_contexts,
            node_key,
            args.responsive_webviews,
            args.webview_creation_backpressure,
            args.lifecycle_intents,
        );
    }
}
