/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::rc::Rc;

use egui_tiles::Tree;
use servo::OffscreenRenderingContext;

use crate::app::GraphBrowserApp;
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::graph::NodeKey;

pub(crate) fn collect_tile_invariant_violations(
    tiles_tree: &Tree<TileKind>,
    graph_app: &GraphBrowserApp,
    tile_rendering_contexts: &HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
) -> Vec<String> {
    let mut violations = Vec::new();
    for node_key in tile_runtime::all_webview_tile_nodes(tiles_tree) {
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
