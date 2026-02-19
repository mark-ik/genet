/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use egui_tiles::Tree;
use servo::{OffscreenRenderingContext, WebViewId, WindowRenderingContext};

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::desktop::webview_backpressure::{self, WebviewCreationBackpressureState};
use crate::desktop::webview_controller;
use crate::graph::NodeKey;
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

pub(crate) struct RuntimeReconcileArgs<'a> {
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
    pub(crate) frame_intents: &'a mut Vec<GraphIntent>,
}

pub(crate) fn reconcile_runtime(args: RuntimeReconcileArgs<'_>) {
    if args.graph_app.graph.node_count() == 0 {
        args.graph_app.active_webview_nodes.clear();
        args.webview_creation_backpressure.clear();
        tile_runtime::reset_runtime_webview_state(
            args.tiles_tree,
            args.tile_rendering_contexts,
            args.tile_favicon_textures,
            args.favicon_textures,
        );
    }

    tile_runtime::prune_stale_webview_tiles(
        args.tiles_tree,
        args.graph_app,
        args.window,
        args.tile_rendering_contexts,
        args.frame_intents,
    );
    args.tile_favicon_textures
        .retain(|node_key, _| args.graph_app.graph.get_node(*node_key).is_some());

    let has_webview_tiles = tile_runtime::has_any_webview_tiles(args.tiles_tree);
    if has_webview_tiles {
        args.frame_intents
            .extend(webview_controller::sync_to_graph_intents(
                args.graph_app,
                args.window,
            ));
        webview_backpressure::reconcile_webview_creation_backpressure(
            args.graph_app,
            args.window,
            args.responsive_webviews,
            args.webview_creation_backpressure,
            args.frame_intents,
        );

        // Keep WebView/context mappings complete for all tile nodes (not only visible ones).
        for node_key in tile_runtime::all_webview_tile_nodes(args.tiles_tree) {
            webview_backpressure::ensure_webview_for_node(
                args.graph_app,
                args.window,
                args.app_state,
                args.rendering_context,
                args.window_rendering_context,
                args.tile_rendering_contexts,
                node_key,
                args.responsive_webviews,
                args.webview_creation_backpressure,
                args.frame_intents,
            );
        }
    } else {
        args.webview_creation_backpressure.clear();
    }
}
