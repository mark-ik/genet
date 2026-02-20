/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use egui_tiles::Tree;
use servo::{OffscreenRenderingContext, WebViewId, WindowRenderingContext};
use sysinfo::System;

use crate::app::{GraphBrowserApp, GraphIntent, MemoryPressureLevel};
use crate::desktop::tile_compositor;
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::desktop::webview_backpressure::{self, WebviewCreationBackpressureState};
use crate::desktop::webview_controller;
use crate::graph::{NodeKey, NodeLifecycle};
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

fn sample_memory_pressure() -> (MemoryPressureLevel, u64, u64) {
    let mut system = System::new();
    system.refresh_memory();

    let total_bytes = system.total_memory();
    let available_bytes = system.available_memory();
    let total_mib = total_bytes / (1024 * 1024);
    let available_mib = available_bytes / (1024 * 1024);

    if total_bytes == 0 {
        return (MemoryPressureLevel::Unknown, available_mib, total_mib);
    }

    let available_pct = available_bytes as f64 / total_bytes as f64;
    let level = if available_mib <= 512 || available_pct <= 0.08 {
        MemoryPressureLevel::Critical
    } else if available_mib <= 1024 || available_pct <= 0.15 {
        MemoryPressureLevel::Warning
    } else {
        MemoryPressureLevel::Normal
    };
    (level, available_mib, total_mib)
}

fn pressure_adjusted_active_limit(base_limit: usize, level: MemoryPressureLevel) -> usize {
    match level {
        MemoryPressureLevel::Unknown | MemoryPressureLevel::Normal => base_limit,
        MemoryPressureLevel::Warning => base_limit.saturating_sub(1).max(1),
        MemoryPressureLevel::Critical => 1,
    }
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
    for node_key in args.graph_app.take_warm_cache_evictions() {
        if let Some(webview_id) = args.graph_app.get_webview_for_node(node_key) {
            args.window.close_webview(webview_id);
            args.frame_intents
                .push(GraphIntent::UnmapWebview { webview_id });
        }
        args.tile_rendering_contexts.remove(&node_key);
        args.frame_intents
            .push(GraphIntent::DemoteNodeToCold { key: node_key });
    }
    args.tile_favicon_textures
        .retain(|node_key, _| args.graph_app.graph.get_node(*node_key).is_some());

    let (memory_pressure_level, available_mib, total_mib) = sample_memory_pressure();
    args.graph_app
        .set_memory_pressure_status(memory_pressure_level, available_mib, total_mib);

    let tile_nodes = tile_runtime::all_webview_tile_nodes(args.tiles_tree);
    let active_tile_nodes: HashSet<NodeKey> =
        tile_compositor::active_webview_tile_rects(args.tiles_tree)
            .into_iter()
            .map(|(node_key, _)| node_key)
            .collect();
    let has_webview_tiles = !tile_nodes.is_empty();
    for node_key in active_tile_nodes.iter().copied() {
        let should_promote = args
            .graph_app
            .graph
            .get_node(node_key)
            .map(|node| node.lifecycle != NodeLifecycle::Active)
            .unwrap_or(false);
        if should_promote && args.graph_app.get_node_crash_state(node_key).is_none() {
            args.graph_app.promote_node_to_active(node_key);
        }
    }
    let prewarm_selected_node = args
        .graph_app
        .get_single_selected_node()
        .filter(|node_key| !active_tile_nodes.contains(node_key))
        .filter(|node_key| args.graph_app.get_webview_for_node(*node_key).is_none())
        .filter(|node_key| args.graph_app.get_node_crash_state(*node_key).is_none());
    if let Some(node_key) = prewarm_selected_node
        && args
            .graph_app
            .graph
            .get_node(node_key)
            .map(|node| node.lifecycle != NodeLifecycle::Active)
            .unwrap_or(false)
    {
        args.graph_app.promote_node_to_active(node_key);
    }

    if has_webview_tiles {
        args.frame_intents
            .extend(webview_controller::sync_to_graph_intents(
                args.graph_app,
                args.window,
            ));
    }

    if has_webview_tiles || prewarm_selected_node.is_some() {
        webview_backpressure::reconcile_webview_creation_backpressure(
            args.graph_app,
            args.window,
            args.responsive_webviews,
            args.webview_creation_backpressure,
            args.frame_intents,
        );

        // Keep WebView/context mappings complete for active tile nodes and prewarm target.
        for node_key in active_tile_nodes.iter().copied() {
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
        if let Some(node_key) = prewarm_selected_node {
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

        let mut protected_active_nodes = active_tile_nodes.clone();
        if let Some(node_key) = prewarm_selected_node {
            protected_active_nodes.insert(node_key);
        }

        let base_active_limit = args.graph_app.active_webview_limit();
        let pressure_limit =
            pressure_adjusted_active_limit(base_active_limit, memory_pressure_level);
        if pressure_limit < base_active_limit {
            for node_key in args
                .graph_app
                .take_active_webview_evictions_with_limit(pressure_limit, &protected_active_nodes)
            {
                if let Some(webview_id) = args.graph_app.get_webview_for_node(node_key) {
                    args.window.close_webview(webview_id);
                    args.frame_intents
                        .push(GraphIntent::UnmapWebview { webview_id });
                }
                args.tile_rendering_contexts.remove(&node_key);
                args.frame_intents
                    .push(GraphIntent::DemoteNodeToCold { key: node_key });
            }
        }
        for node_key in args
            .graph_app
            .take_active_webview_evictions(&protected_active_nodes)
        {
            args.frame_intents
                .push(GraphIntent::DemoteNodeToWarm { key: node_key });
        }
    } else {
        args.webview_creation_backpressure.clear();
    }
}
