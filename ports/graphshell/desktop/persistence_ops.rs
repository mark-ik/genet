/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use egui_tiles::{Tiles, Tree};
use log::warn;
use servo::{OffscreenRenderingContext, WebViewId};
use uuid::Uuid;

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::desktop::webview_controller;
use crate::graph::NodeKey;
use crate::window::ServoShellWindow;

pub(crate) fn restore_tiles_tree_from_persistence(graph_app: &GraphBrowserApp) -> Tree<TileKind> {
    let mut tiles = Tiles::default();
    let graph_tile_id = tiles.insert_pane(TileKind::Graph);
    let mut tiles_tree = Tree::new("graphshell_tiles", graph_tile_id, tiles);
    if let Some(layout_json) = graph_app.load_tile_layout_json()
        && let Ok(mut restored_tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json)
    {
        tile_runtime::prune_stale_webview_tile_keys_only(&mut restored_tree, graph_app);
        if restored_tree.root().is_some() {
            tiles_tree = restored_tree;
        }
    }
    tiles_tree
}

fn workspace_nodes_from_tree(tree: &Tree<TileKind>) -> Vec<NodeKey> {
    tree.tiles
        .iter()
        .filter_map(|(_, tile)| match tile {
            egui_tiles::Tile::Pane(TileKind::WebView(key)) => Some(*key),
            _ => None,
        })
        .collect()
}

/// Rebuild the UUID-keyed workspace membership index from persisted named workspace layouts.
///
/// Reserved autosave/session workspace keys are intentionally excluded so routing decisions
/// operate on user-meaningful named workspaces.
pub(crate) fn build_membership_index_from_layouts(
    graph_app: &GraphBrowserApp,
) -> HashMap<Uuid, BTreeSet<String>> {
    let mut index: HashMap<Uuid, BTreeSet<String>> = HashMap::new();

    for workspace_name in graph_app.list_workspace_layout_names() {
        if GraphBrowserApp::is_reserved_workspace_layout_name(&workspace_name) {
            continue;
        }
        let Some(layout_json) = graph_app.load_workspace_layout_json(&workspace_name) else {
            continue;
        };
        let Ok(mut tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json) else {
            warn!("Skipping workspace '{workspace_name}': invalid layout json");
            continue;
        };
        tile_runtime::prune_stale_webview_tile_keys_only(&mut tree, graph_app);
        for node_key in workspace_nodes_from_tree(&tree) {
            let Some(node) = graph_app.graph.get_node(node_key) else {
                continue;
            };
            index
                .entry(node.id)
                .or_default()
                .insert(workspace_name.clone());
        }
    }

    index
}

pub(crate) fn switch_persistence_store(
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
    tile_runtime::reset_runtime_webview_state(
        tiles_tree,
        tile_rendering_contexts,
        tile_favicon_textures,
        favicon_textures,
    );

    graph_app.switch_persistence_dir(data_dir)?;
    if let Some(secs) = snapshot_interval_secs {
        graph_app.set_snapshot_interval_secs(secs)?;
    }
    *tiles_tree = restore_tiles_tree_from_persistence(graph_app);
    let membership_index = build_membership_index_from_layouts(graph_app);
    graph_app.init_membership_index(membership_index);
    Ok(())
}

/// Delete named workspaces that become empty after stale-node pruning.
pub(crate) fn prune_empty_named_workspaces(graph_app: &mut GraphBrowserApp) -> usize {
    let mut names_to_delete = Vec::new();
    for workspace_name in graph_app.list_workspace_layout_names() {
        if GraphBrowserApp::is_reserved_workspace_layout_name(&workspace_name) {
            continue;
        }
        let Some(layout_json) = graph_app.load_workspace_layout_json(&workspace_name) else {
            continue;
        };
        let Ok(mut tree) = serde_json::from_str::<Tree<TileKind>>(&layout_json) else {
            warn!("Skipping workspace '{workspace_name}': invalid layout json");
            continue;
        };
        tile_runtime::prune_stale_webview_tile_keys_only(&mut tree, graph_app);
        if workspace_nodes_from_tree(&tree).is_empty() {
            names_to_delete.push(workspace_name);
        }
    }
    let mut deleted = 0usize;
    for name in names_to_delete {
        if graph_app.delete_workspace_layout(&name).is_ok() {
            deleted += 1;
        }
    }
    if deleted > 0 {
        let membership_index = build_membership_index_from_layouts(graph_app);
        graph_app.init_membership_index(membership_index);
    }
    deleted
}

/// Keep only the latest N named workspaces by activation recency.
pub(crate) fn keep_latest_named_workspaces(graph_app: &mut GraphBrowserApp, keep: usize) -> usize {
    let mut names: Vec<String> = graph_app
        .list_workspace_layout_names()
        .into_iter()
        .filter(|name| !GraphBrowserApp::is_reserved_workspace_layout_name(name))
        .collect();
    names.sort_by(|a, b| {
        graph_app
            .workspace_recency_seq_for_name(b)
            .cmp(&graph_app.workspace_recency_seq_for_name(a))
            .then_with(|| a.cmp(b))
    });
    let names_to_delete: Vec<String> = names.into_iter().skip(keep).collect();
    let mut deleted = 0usize;
    for name in names_to_delete {
        if graph_app.delete_workspace_layout(&name).is_ok() {
            deleted += 1;
        }
    }
    if deleted > 0 {
        let membership_index = build_membership_index_from_layouts(graph_app);
        graph_app.init_membership_index(membership_index);
    }
    deleted
}

pub(crate) fn parse_data_dir_input(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_tiles::{Tiles, Tree};
    use euclid::default::Point2D;
    use tempfile::TempDir;

    fn workspace_layout_json_with_nodes(node_keys: &[NodeKey]) -> String {
        let mut tiles = Tiles::default();
        let mut children = Vec::new();
        children.push(tiles.insert_pane(TileKind::Graph));
        for node_key in node_keys {
            children.push(tiles.insert_pane(TileKind::WebView(*node_key)));
        }
        let root = if children.len() == 1 {
            children[0]
        } else {
            tiles.insert_tab_tile(children)
        };
        let tree = Tree::new("workspace_test", root, tiles);
        serde_json::to_string(&tree).expect("workspace layout should serialize")
    }

    #[test]
    fn test_build_membership_index_from_layouts_skips_reserved_and_stale_nodes() {
        let dir = TempDir::new().unwrap();
        let mut app = GraphBrowserApp::new_from_dir(dir.path().to_path_buf());
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("https://b.example".into(), Point2D::new(1.0, 0.0));
        let a_id = app.graph.get_node(a).unwrap().id;
        let b_id = app.graph.get_node(b).unwrap().id;
        let stale = NodeKey::new(999_999);

        app.save_workspace_layout_json(
            "workspace-alpha",
            &workspace_layout_json_with_nodes(&[a, b]),
        );
        app.save_workspace_layout_json("workspace-beta", &workspace_layout_json_with_nodes(&[b]));
        app.save_workspace_layout_json(
            "workspace-stale",
            &workspace_layout_json_with_nodes(&[stale]),
        );
        app.save_workspace_layout_json("latest", &workspace_layout_json_with_nodes(&[a]));

        let index = build_membership_index_from_layouts(&app);
        assert_eq!(
            index.get(&a_id),
            Some(&BTreeSet::from(["workspace-alpha".to_string()]))
        );
        assert_eq!(
            index.get(&b_id),
            Some(&BTreeSet::from([
                "workspace-alpha".to_string(),
                "workspace-beta".to_string()
            ]))
        );
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn test_prune_empty_named_workspaces_rebuilds_membership_index() {
        let dir = TempDir::new().unwrap();
        let mut app = GraphBrowserApp::new_from_dir(dir.path().to_path_buf());
        let live = app.add_node_and_sync("https://live.example".into(), Point2D::new(0.0, 0.0));
        let live_id = app.graph.get_node(live).unwrap().id;
        let stale = NodeKey::new(888_888);

        app.save_workspace_layout_json(
            "workspace-keep",
            &workspace_layout_json_with_nodes(&[live]),
        );
        app.save_workspace_layout_json(
            "workspace-empty",
            &workspace_layout_json_with_nodes(&[stale]),
        );
        app.init_membership_index(HashMap::from([(
            live_id,
            BTreeSet::from(["workspace-keep".to_string(), "workspace-empty".to_string()]),
        )]));

        let deleted = prune_empty_named_workspaces(&mut app);
        assert_eq!(deleted, 1);
        assert!(app.load_workspace_layout_json("workspace-empty").is_none());
        assert!(app.load_workspace_layout_json("workspace-keep").is_some());
        assert_eq!(
            app.membership_for_node(live_id),
            &BTreeSet::from(["workspace-keep".to_string()])
        );
    }

    #[test]
    fn test_keep_latest_named_workspaces_rebuilds_membership_index() {
        let dir = TempDir::new().unwrap();
        let mut app = GraphBrowserApp::new_from_dir(dir.path().to_path_buf());
        let node = app.add_node_and_sync("https://node.example".into(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(node).unwrap().id;

        app.save_workspace_layout_json("workspace-old", &workspace_layout_json_with_nodes(&[node]));
        app.save_workspace_layout_json("workspace-mid", &workspace_layout_json_with_nodes(&[node]));
        app.save_workspace_layout_json("workspace-new", &workspace_layout_json_with_nodes(&[node]));
        app.note_workspace_activated("workspace-old", [node]);
        app.note_workspace_activated("workspace-mid", [node]);
        app.note_workspace_activated("workspace-new", [node]);

        let deleted = keep_latest_named_workspaces(&mut app, 1);
        assert_eq!(deleted, 2);
        assert!(app.load_workspace_layout_json("workspace-old").is_none());
        assert!(app.load_workspace_layout_json("workspace-mid").is_none());
        assert!(app.load_workspace_layout_json("workspace-new").is_some());
        assert_eq!(
            app.membership_for_node(node_id),
            &BTreeSet::from(["workspace-new".to_string()])
        );
    }
}
