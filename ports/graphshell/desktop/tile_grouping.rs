/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;

use egui_tiles::{Container, Tile, TileId, Tree};

use crate::app::GraphIntent;
use crate::graph::NodeKey;

fn nearest_tabs_container_for_tile(
    tiles_tree: &Tree<super::tile_kind::TileKind>,
    mut tile_id: TileId,
) -> Option<TileId> {
    while let Some(parent_id) = tiles_tree.tiles.parent_of(tile_id) {
        if matches!(
            tiles_tree.tiles.get(parent_id),
            Some(Tile::Container(Container::Tabs(_)))
        ) {
            return Some(parent_id);
        }
        tile_id = parent_id;
    }
    None
}

pub(crate) fn webview_tab_group_memberships(
    tiles_tree: &Tree<super::tile_kind::TileKind>,
) -> HashMap<NodeKey, TileId> {
    let mut memberships = HashMap::new();
    for (tile_id, tile) in tiles_tree.tiles.iter() {
        let Tile::Pane(super::tile_kind::TileKind::WebView(node_key)) = tile else {
            continue;
        };
        if let Some(group_id) = nearest_tabs_container_for_tile(tiles_tree, *tile_id) {
            memberships.insert(*node_key, group_id);
        }
    }
    memberships
}

pub(crate) fn tab_group_nodes(
    tiles_tree: &Tree<super::tile_kind::TileKind>,
) -> HashMap<TileId, Vec<NodeKey>> {
    let mut groups: HashMap<TileId, Vec<NodeKey>> = HashMap::new();
    for (tile_id, tile) in tiles_tree.tiles.iter() {
        let Tile::Pane(super::tile_kind::TileKind::WebView(node_key)) = tile else {
            continue;
        };
        if let Some(group_id) = nearest_tabs_container_for_tile(tiles_tree, *tile_id) {
            groups.entry(group_id).or_default().push(*node_key);
        }
    }
    groups
}

pub(crate) fn user_grouped_intents_for_tab_group_moves(
    tab_groups_before: &HashMap<NodeKey, TileId>,
    tab_groups_after: &HashMap<NodeKey, TileId>,
    tab_group_nodes_after: &HashMap<TileId, Vec<NodeKey>>,
    moved_nodes: &std::collections::HashSet<NodeKey>,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    for (node_key, before_group) in tab_groups_before {
        if !moved_nodes.contains(node_key) {
            continue;
        }
        let Some(after_group) = tab_groups_after.get(node_key) else {
            continue;
        };
        if before_group == after_group {
            continue;
        }
        let Some(peers) = tab_group_nodes_after.get(after_group) else {
            continue;
        };
        if let Some(anchor) = peers.iter().copied().find(|peer| *peer != *node_key) {
            intents.push(GraphIntent::CreateUserGroupedEdge {
                from: *node_key,
                to: anchor,
            });
        }
    }
    intents
}
