/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui_tiles::Tree;
use servo::{OffscreenRenderingContext, WebViewId, WindowRenderingContext};

use super::tile_behavior::PendingOpenMode;
use super::tile_compositor;
use super::tile_kind::TileKind;
use super::tile_post_render;
use super::tile_runtime;
use super::tile_view_ops::{self, TileOpenMode};
use super::webview_backpressure::{self, WebviewCreationBackpressureState};
use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::NodeKey;
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

pub(crate) struct TileRenderPassArgs<'a> {
    pub ctx: &'a egui::Context,
    pub graph_app: &'a mut GraphBrowserApp,
    pub window: &'a ServoShellWindow,
    pub tiles_tree: &'a mut Tree<TileKind>,
    pub tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub graph_search_matches: &'a HashSet<NodeKey>,
    pub active_search_match: Option<NodeKey>,
    pub graph_search_filter_mode: bool,
    pub search_query_active: bool,
    pub app_state: &'a Option<Rc<RunningAppState>>,
    pub rendering_context: &'a Rc<OffscreenRenderingContext>,
    pub window_rendering_context: &'a Rc<WindowRenderingContext>,
    pub responsive_webviews: &'a HashSet<WebViewId>,
    pub webview_creation_backpressure: &'a mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    pub focused_webview_hint: &'a mut Option<WebViewId>,
    pub graph_surface_focused: bool,
    pub focus_ring_webview_id: &'a mut Option<WebViewId>,
    pub focus_ring_started_at: &'a mut Option<Instant>,
    pub focus_ring_duration: Duration,
}

fn open_mode_from_pending(mode: PendingOpenMode) -> TileOpenMode {
    match mode {
        PendingOpenMode::SplitHorizontal => TileOpenMode::SplitHorizontal,
    }
}

pub(crate) fn run_tile_render_pass(args: TileRenderPassArgs<'_>) -> Vec<GraphIntent> {
    let TileRenderPassArgs {
        ctx,
        graph_app,
        window,
        tiles_tree,
        tile_rendering_contexts,
        tile_favicon_textures,
        graph_search_matches,
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
    } = args;

    let mut post_render_intents = Vec::new();
    let mut pending_open_nodes = Vec::new();
    let mut pending_closed_nodes = Vec::new();
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(egui::Color32::from_rgb(20, 20, 25)))
        .show(ctx, |ui| {
            let outputs = tile_post_render::render_tile_tree_and_collect_outputs(
                ui,
                tiles_tree,
                graph_app,
                tile_favicon_textures,
                graph_search_matches,
                active_search_match,
                graph_search_filter_mode,
                search_query_active,
            );
            pending_open_nodes.extend(outputs.pending_open_nodes);
            pending_closed_nodes.extend(outputs.pending_closed_nodes);
            post_render_intents.extend(outputs.post_render_intents);
        });

    for open in pending_open_nodes {
        tile_view_ops::open_or_focus_webview_tile_with_mode(
            tiles_tree,
            open.key,
            open_mode_from_pending(open.mode),
        );
    }
    for node_key in pending_closed_nodes {
        tile_runtime::close_webview_for_node(
            graph_app,
            window,
            tile_rendering_contexts,
            node_key,
            &mut post_render_intents,
        );
    }

    for node_key in tile_post_render::mapped_nodes_without_tiles(graph_app, tiles_tree) {
        tile_runtime::close_webview_for_node(
            graph_app,
            window,
            tile_rendering_contexts,
            node_key,
            &mut post_render_intents,
        );
    }

    let active_tile_rects = tile_compositor::active_webview_tile_rects(tiles_tree);
    for (node_key, _) in active_tile_rects.iter().copied() {
        webview_backpressure::ensure_webview_for_node(
            graph_app,
            window,
            app_state,
            rendering_context,
            window_rendering_context,
            tile_rendering_contexts,
            node_key,
            responsive_webviews,
            webview_creation_backpressure,
            &mut post_render_intents,
        );
    }
    let focused_webview_id = if graph_surface_focused {
        *focused_webview_hint = None;
        None
    } else {
        tile_compositor::activate_focused_webview_for_frame(
            window,
            tiles_tree,
            graph_app,
            focused_webview_hint,
        );
        let focused_webview_id = tile_compositor::focused_webview_id_for_tree(
            tiles_tree,
            graph_app,
            *focused_webview_hint,
        );
        *focused_webview_hint = focused_webview_id;
        focused_webview_id
    };
    if *focus_ring_webview_id != focused_webview_id {
        *focus_ring_webview_id = focused_webview_id;
        *focus_ring_started_at = focused_webview_id.map(|_| Instant::now());
    }

    let focus_ring_alpha = if *focus_ring_webview_id == focused_webview_id {
        focus_ring_started_at
            .as_ref()
            .map(|started| {
                let elapsed = started.elapsed();
                if elapsed >= focus_ring_duration {
                    0.0
                } else {
                    1.0 - (elapsed.as_secs_f32() / focus_ring_duration.as_secs_f32())
                }
            })
            .unwrap_or(0.0)
    } else {
        0.0
    };

    tile_compositor::composite_active_webview_tiles(
        ctx,
        window,
        graph_app,
        tile_rendering_contexts,
        active_tile_rects,
        focused_webview_id,
        focus_ring_alpha,
    );

    post_render_intents
}
