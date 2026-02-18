/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use egui_tiles::{Tiles, Tree};
use servo::{OffscreenRenderingContext, WebViewId};

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
    Ok(())
}

pub(crate) fn parse_data_dir_input(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}
