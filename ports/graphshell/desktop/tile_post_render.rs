/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};

use egui_tiles::Tree;

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::tile_behavior::{GraphshellTileBehavior, PendingOpenNode};
use crate::desktop::tile_grouping;
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::graph::NodeKey;

pub(crate) struct TileRenderOutputs {
    pub(crate) pending_open_nodes: Vec<PendingOpenNode>,
    pub(crate) pending_closed_nodes: Vec<NodeKey>,
    pub(crate) post_render_intents: Vec<GraphIntent>,
}

pub(crate) fn render_tile_tree_and_collect_outputs(
    ui: &mut egui::Ui,
    tiles_tree: &mut Tree<TileKind>,
    graph_app: &mut GraphBrowserApp,
    tile_favicon_textures: &mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    search_matches: &HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    graph_search_filter_mode: bool,
    search_query_active: bool,
) -> TileRenderOutputs {
    let tab_groups_before = tile_grouping::webview_tab_group_memberships(tiles_tree);
    let mut behavior = GraphshellTileBehavior::new(
        graph_app,
        tile_favicon_textures,
        search_matches,
        active_search_match,
        graph_search_filter_mode,
        search_query_active,
    );
    tiles_tree.ui(&mut behavior, ui);

    let pending_open_nodes = behavior.take_pending_open_nodes();
    let pending_closed_nodes = behavior.take_pending_closed_nodes();
    let tab_drag_stopped_nodes = behavior.take_pending_tab_drag_stopped_nodes();
    let mut post_render_intents = behavior.take_pending_graph_intents();

    let tab_groups_after = tile_grouping::webview_tab_group_memberships(tiles_tree);
    let tab_group_nodes_after = tile_grouping::tab_group_nodes(tiles_tree);
    post_render_intents.extend(tile_grouping::user_grouped_intents_for_tab_group_moves(
        &tab_groups_before,
        &tab_groups_after,
        &tab_group_nodes_after,
        &tab_drag_stopped_nodes,
    ));

    TileRenderOutputs {
        pending_open_nodes,
        pending_closed_nodes,
        post_render_intents,
    }
}

pub(crate) fn mapped_nodes_without_tiles(
    graph_app: &GraphBrowserApp,
    tiles_tree: &Tree<TileKind>,
) -> Vec<NodeKey> {
    let tile_nodes = tile_runtime::all_webview_tile_nodes(tiles_tree);
    graph_app
        .webview_node_mappings()
        .map(|(_, node_key)| node_key)
        .filter(|node_key| !tile_nodes.contains(node_key))
        .collect()
}
