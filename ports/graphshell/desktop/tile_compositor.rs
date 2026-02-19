/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use egui::{Id, LayerId, PaintCallback, Stroke, StrokeKind};
use egui_glow::CallbackFn;
use egui_tiles::{Tile, Tree};
use euclid::{Point2D, Rect, Scale, Size2D};
use log::warn;
use servo::{
    DeviceIndependentPixel, DevicePixel, OffscreenRenderingContext, RenderingContext, WebViewId,
};

use crate::app::GraphBrowserApp;
use crate::desktop::tile_kind::TileKind;
use crate::graph::NodeKey;
use crate::window::ServoShellWindow;

pub(crate) fn active_webview_tile_rects(tiles_tree: &Tree<TileKind>) -> Vec<(NodeKey, egui::Rect)> {
    let mut tile_rects = Vec::new();
    for tile_id in tiles_tree.active_tiles() {
        if let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles_tree.tiles.get(tile_id)
            && let Some(rect) = tiles_tree.tiles.rect(tile_id)
        {
            tile_rects.push((*node_key, rect));
        }
    }
    tile_rects
}

pub(crate) fn focused_webview_id_for_tree(
    tiles_tree: &Tree<TileKind>,
    graph_app: &GraphBrowserApp,
    focused_hint: Option<WebViewId>,
) -> Option<WebViewId> {
    if let Some(hint) = focused_hint {
        let hint_present_in_tree = tiles_tree.tiles.iter().any(|(_, tile)| {
            matches!(
                tile,
                Tile::Pane(TileKind::WebView(node_key))
                    if graph_app.get_webview_for_node(*node_key) == Some(hint)
            )
        });
        if hint_present_in_tree {
            return Some(hint);
        }
    }

    active_webview_tile_node(tiles_tree)
        .and_then(|node_key| graph_app.get_webview_for_node(node_key))
}

pub(crate) fn webview_for_frame_activation(
    tiles_tree: &Tree<TileKind>,
    graph_app: &GraphBrowserApp,
    focused_hint: Option<WebViewId>,
) -> Option<WebViewId> {
    focused_webview_id_for_tree(tiles_tree, graph_app, focused_hint).or_else(|| {
        active_webview_tile_rects(tiles_tree)
            .first()
            .and_then(|(node_key, _)| graph_app.get_webview_for_node(*node_key))
    })
}

pub(crate) fn activate_focused_webview_for_frame(
    window: &ServoShellWindow,
    tiles_tree: &Tree<TileKind>,
    graph_app: &GraphBrowserApp,
    focused_webview_hint: &mut Option<WebViewId>,
) {
    if let Some(wv_id) = webview_for_frame_activation(tiles_tree, graph_app, *focused_webview_hint)
    {
        *focused_webview_hint = Some(wv_id);
        window.activate_webview(wv_id);
    }
}

pub(crate) fn composite_active_webview_tiles(
    ctx: &egui::Context,
    window: &ServoShellWindow,
    graph_app: &GraphBrowserApp,
    tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    active_tile_rects: Vec<(NodeKey, egui::Rect)>,
    focused_webview_id: Option<WebViewId>,
    focus_ring_alpha: f32,
) {
    // Keep focus ring below popup/panel overlays (palette, dialogs) so it never occludes UI.
    let focus_ring_layer = LayerId::new(egui::Order::Background, Id::new("graphshell_focus_ring"));
    let hover_ring_layer = LayerId::new(egui::Order::Background, Id::new("graphshell_hover_ring"));
    let scale = Scale::<_, DeviceIndependentPixel, DevicePixel>::new(ctx.pixels_per_point());
    let hover_pos = ctx.input(|i| i.pointer.hover_pos());
    let mut hovered_webview_id: Option<WebViewId> = None;
    if let Some(pos) = hover_pos {
        for (node_key, tile_rect) in active_tile_rects.iter().copied() {
            if !tile_rect.contains(pos) {
                continue;
            }
            hovered_webview_id = graph_app.get_webview_for_node(node_key);
            if hovered_webview_id.is_some() {
                break;
            }
        }
    }
    for (node_key, tile_rect) in active_tile_rects {
        let size = Size2D::new(tile_rect.width(), tile_rect.height()) * scale;
        let target_size = PhysicalSize::new(
            size.width.max(1.0).round() as u32,
            size.height.max(1.0).round() as u32,
        );

        let Some(render_context) = tile_rendering_contexts.get(&node_key).cloned() else {
            continue;
        };

        if render_context.size() != target_size {
            render_context.resize(target_size);
        }

        let Some(webview_id) = graph_app.get_webview_for_node(node_key) else {
            continue;
        };
        let Some(webview) = window.webview_by_id(webview_id) else {
            continue;
        };
        if webview.size() != size {
            webview.resize(target_size);
        }

        if let Err(e) = render_context.make_current() {
            warn!("Failed to make tile rendering context current: {e:?}");
            continue;
        }
        render_context.prepare_for_rendering();
        webview.paint();
        render_context.present();

        if let Some(render_to_parent) = render_context.render_to_parent_callback() {
            ctx.layer_painter(LayerId::background()).add(PaintCallback {
                rect: tile_rect,
                callback: Arc::new(CallbackFn::new(move |info, painter| {
                    let clip = info.viewport_in_pixels();
                    let rect_in_parent = Rect::new(
                        Point2D::new(clip.left_px, clip.from_bottom_px),
                        Size2D::new(clip.width_px, clip.height_px),
                    );
                    render_to_parent(painter.gl(), rect_in_parent)
                })),
            });
        }

        if focused_webview_id == Some(webview_id) && focus_ring_alpha > 0.0 {
            let alpha = (focus_ring_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            ctx.layer_painter(focus_ring_layer).rect_stroke(
                tile_rect.shrink(1.0),
                4.0,
                Stroke::new(
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(120, 200, 255, alpha),
                ),
                StrokeKind::Inside,
            );
        } else if hovered_webview_id == Some(webview_id) {
            // Temporary hover affordance (no input ownership change).
            ctx.layer_painter(hover_ring_layer).rect_stroke(
                tile_rect.shrink(1.0),
                4.0,
                Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_unmultiplied(180, 180, 190, 180),
                ),
                StrokeKind::Inside,
            );
        }
    }
}

fn active_webview_tile_node(tiles_tree: &Tree<TileKind>) -> Option<NodeKey> {
    tiles_tree
        .active_tiles()
        .into_iter()
        .find_map(|tile_id| match tiles_tree.tiles.get(tile_id) {
            Some(Tile::Pane(TileKind::WebView(node_key))) => Some(*node_key),
            _ => None,
        })
}
