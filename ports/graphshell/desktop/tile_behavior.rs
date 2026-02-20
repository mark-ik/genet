/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Initial egui_tiles behavior wiring.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use egui::{Color32, Id, Response, Sense, Stroke, TextStyle, Ui, Vec2, WidgetText, vec2};
use egui_tiles::{Behavior, Container, SimplificationOptions, TabState, Tile, TileId, Tiles, UiResponse};

use crate::app::{GraphBrowserApp, GraphIntent, SearchDisplayMode};
use crate::graph::{NodeKey, NodeLifecycle};
use crate::render;
use crate::render::GraphAction;
use crate::util::truncate_with_ellipsis;

use super::selection_range::inclusive_index_range;
use super::tile_kind::TileKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingOpenMode {
    SplitHorizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingOpenNode {
    pub key: NodeKey,
    pub mode: PendingOpenMode,
}

pub(crate) struct GraphshellTileBehavior<'a> {
    pub graph_app: &'a mut GraphBrowserApp,
    tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    search_matches: &'a HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    search_filter_mode: bool,
    search_query_active: bool,
    pending_open_nodes: Vec<PendingOpenNode>,
    pending_closed_nodes: Vec<NodeKey>,
    pending_graph_intents: Vec<GraphIntent>,
    pending_tab_drag_stopped_nodes: HashSet<NodeKey>,
}

impl<'a> GraphshellTileBehavior<'a> {
    pub fn new(
        graph_app: &'a mut GraphBrowserApp,
        tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
        search_matches: &'a HashSet<NodeKey>,
        active_search_match: Option<NodeKey>,
        search_filter_mode: bool,
        search_query_active: bool,
    ) -> Self {
        Self {
            graph_app,
            tile_favicon_textures,
            search_matches,
            active_search_match,
            search_filter_mode,
            search_query_active,
            pending_open_nodes: Vec::new(),
            pending_closed_nodes: Vec::new(),
            pending_graph_intents: Vec::new(),
            pending_tab_drag_stopped_nodes: HashSet::new(),
        }
    }

    pub fn take_pending_open_nodes(&mut self) -> Vec<PendingOpenNode> {
        std::mem::take(&mut self.pending_open_nodes)
    }

    pub fn take_pending_closed_nodes(&mut self) -> Vec<NodeKey> {
        std::mem::take(&mut self.pending_closed_nodes)
    }

    pub fn take_pending_graph_intents(&mut self) -> Vec<GraphIntent> {
        std::mem::take(&mut self.pending_graph_intents)
    }

    pub fn take_pending_tab_drag_stopped_nodes(&mut self) -> HashSet<NodeKey> {
        std::mem::take(&mut self.pending_tab_drag_stopped_nodes)
    }

    fn hash_favicon(width: u32, height: u32, rgba: &[u8]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        rgba.hash(&mut hasher);
        hasher.finish()
    }

    fn favicon_texture_id(&mut self, ui: &Ui, node_key: NodeKey) -> Option<egui::TextureId> {
        let (favicon_rgba, favicon_width, favicon_height) = {
            let node = self.graph_app.graph.get_node(node_key)?;
            (
                node.favicon_rgba.clone()?,
                node.favicon_width as usize,
                node.favicon_height as usize,
            )
        };
        if favicon_width == 0 || favicon_height == 0 {
            self.tile_favicon_textures.remove(&node_key);
            return None;
        }
        let expected_len = favicon_width * favicon_height * 4;
        if favicon_rgba.len() != expected_len {
            self.tile_favicon_textures.remove(&node_key);
            return None;
        }

        let favicon_hash =
            Self::hash_favicon(favicon_width as u32, favicon_height as u32, &favicon_rgba);

        let handle = if let Some((cached_hash, handle)) = self.tile_favicon_textures.get(&node_key)
        {
            if *cached_hash == favicon_hash {
                handle.clone()
            } else {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [favicon_width, favicon_height],
                    &favicon_rgba,
                );
                let handle = ui.ctx().load_texture(
                    format!("tile-favicon-{node_key:?}-{favicon_hash}"),
                    image,
                    Default::default(),
                );
                self.tile_favicon_textures
                    .insert(node_key, (favicon_hash, handle.clone()));
                handle
            }
        } else {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [favicon_width, favicon_height],
                &favicon_rgba,
            );
            let handle = ui.ctx().load_texture(
                format!("tile-favicon-{node_key:?}-{favicon_hash}"),
                image,
                Default::default(),
            );
            self.tile_favicon_textures
                .insert(node_key, (favicon_hash, handle.clone()));
            handle
        };

        Some(handle.id())
    }

    fn should_detach_tab_on_drag_stop(ui: &Ui, tab_rect: egui::Rect) -> bool {
        // Treat release clearly outside the tab strip band as "detach tab to split".
        // Horizontal motion within the tab strip should keep normal tab reorder/group behavior.
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return false;
        };
        let detach_band_margin = 12.0;
        pointer.y < tab_rect.top() - detach_band_margin
            || pointer.y > tab_rect.bottom() + detach_band_margin
    }

    fn tab_group_node_order_for_tile(tiles: &Tiles<TileKind>, tile_id: TileId) -> Option<Vec<NodeKey>> {
        for (_, tile) in tiles.iter() {
            let Tile::Container(Container::Tabs(tabs)) = tile else {
                continue;
            };
            if !tabs.children.contains(&tile_id) {
                continue;
            }
            let mut out = Vec::new();
            for child_id in &tabs.children {
                if let Some(Tile::Pane(TileKind::WebView(key))) = tiles.get(*child_id) {
                    out.push(*key);
                }
            }
            return Some(out);
        }
        None
    }
}

impl<'a> Behavior<TileKind> for GraphshellTileBehavior<'a> {
    fn simplification_options(&self) -> SimplificationOptions {
        // Keep a tab container around every pane so split panes always expose
        // a local tab strip for move/merge flows.
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut TileKind) -> UiResponse {
        match pane {
            TileKind::Graph => {
                let actions = render::render_graph_in_ui_collect_actions(
                    ui,
                    self.graph_app,
                    self.search_matches,
                    self.active_search_match,
                    if self.search_filter_mode {
                        SearchDisplayMode::Filter
                    } else {
                        SearchDisplayMode::Highlight
                    },
                    self.search_query_active,
                );
                let multi_select_modifier = ui.input(|i| i.modifiers.ctrl);
                let mut passthrough_actions = Vec::new();

                for action in actions {
                    match action {
                        GraphAction::FocusNode(key) => {
                            self.pending_graph_intents
                                .push(GraphIntent::OpenNodeWorkspaceRouted {
                                    key,
                                    prefer_workspace: None,
                                });
                        },
                        GraphAction::FocusNodeSplit(key) => {
                            if let Some(primary) = self.graph_app.selected_nodes.primary()
                                && primary != key
                            {
                                self.pending_graph_intents.push(
                                    GraphIntent::CreateUserGroupedEdge {
                                        from: primary,
                                        to: key,
                                    },
                                );
                            }
                            self.pending_graph_intents.push(GraphIntent::SelectNode {
                                key,
                                multi_select: multi_select_modifier,
                            });
                            self.pending_open_nodes.push(PendingOpenNode {
                                key,
                                mode: PendingOpenMode::SplitHorizontal,
                            });
                        },
                        other => passthrough_actions.push(other),
                    }
                }

                self.pending_graph_intents
                    .extend(render::intents_from_graph_actions(passthrough_actions));
                render::sync_graph_positions_from_layout(self.graph_app);
                render::render_graph_info_in_ui(ui, self.graph_app);
            },
            TileKind::WebView(node_key) => {
                let Some(node) = self.graph_app.graph.get_node(*node_key) else {
                    ui.label("Missing node for this tile.");
                    return UiResponse::None;
                };
                if let Some(crash) = self.graph_app.get_node_crash_state(*node_key) {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 120),
                        format!("Tab crashed: {}", crash.reason),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Reload").clicked() {
                            self.pending_graph_intents
                                .push(GraphIntent::PromoteNodeToActive { key: *node_key });
                        }
                        if ui.button("Close Tile").clicked() {
                            self.pending_closed_nodes.push(*node_key);
                        }
                    });
                    if crash.has_backtrace {
                        ui.small("Crash reported a backtrace.");
                    }
                    if let Ok(elapsed) =
                        std::time::SystemTime::now().duration_since(crash.crashed_at)
                    {
                        ui.small(format!("Crashed {}s ago", elapsed.as_secs()));
                    }
                    return UiResponse::None;
                }
                if self.graph_app.get_webview_for_node(*node_key).is_none() {
                    let lifecycle_hint = match node.lifecycle {
                        NodeLifecycle::Cold => {
                            "Node is cold. Reactivate to resume browsing in this pane."
                        },
                        NodeLifecycle::Warm => {
                            "Node is warm-cached. Reactivate to attach its cached webview."
                        },
                        NodeLifecycle::Active => {
                            "Node is active but no runtime WebView is mapped yet."
                        },
                    };
                    ui.label(format!("No active WebView for {}", node.url));
                    ui.small(lifecycle_hint);
                    ui.horizontal(|ui| {
                        if ui.button("Reactivate").clicked() {
                            self.pending_graph_intents.push(GraphIntent::SelectNode {
                                key: *node_key,
                                multi_select: false,
                            });
                            self.pending_graph_intents
                                .push(GraphIntent::PromoteNodeToActive { key: *node_key });
                        }
                    });
                }
            },
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &TileKind) -> WidgetText {
        match pane {
            TileKind::Graph => "Graph".into(),
            TileKind::WebView(node_key) => self
                .graph_app
                .graph
                .get_node(*node_key)
                .map(|n| n.title.clone().into())
                .unwrap_or_else(|| format!("Node {:?}", node_key).into()),
        }
    }

    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<TileKind>,
        ui: &mut Ui,
        id: Id,
        tile_id: TileId,
        state: &TabState,
    ) -> Response {
        let close_btn_size = Vec2::splat(self.close_button_outer_size());
        let close_btn_left_padding = 4.0;
        let icon_size = 16.0;
        let icon_spacing = 6.0;
        let x_margin = self.tab_title_spacing(ui.visuals());

        let (title_text, favicon_texture) = match tiles.get(tile_id) {
            Some(Tile::Pane(TileKind::Graph)) => ("Graph".to_string(), None),
            Some(Tile::Pane(TileKind::WebView(node_key))) => {
                let title = self
                    .graph_app
                    .graph
                    .get_node(*node_key)
                    .map(|n| n.title.clone())
                    .unwrap_or_else(|| format!("Node {:?}", node_key));
                let title = truncate_with_ellipsis(&title, 26);
                let favicon = self.favicon_texture_id(ui, *node_key);
                (title, favicon)
            },
            Some(Tile::Container(container)) => (format!("{:?}", container.kind()), None),
            None => ("MISSING TILE".to_string(), None),
        };

        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = WidgetText::from(title_text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            font_id,
        );

        let icon_width = if favicon_texture.is_some() {
            icon_size + icon_spacing
        } else {
            0.0
        };
        let button_width = galley.size().x
            + 2.0 * x_margin
            + icon_width
            + f32::from(state.closable) * (close_btn_left_padding + close_btn_size.x);
        let (_, tab_rect) = ui.allocate_space(vec2(button_width, ui.available_height()));

        let tab_response = ui
            .interact(tab_rect, id, Sense::click_and_drag())
            .on_hover_cursor(self.tab_hover_cursor_icon());

        if tab_response.clicked()
            && let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles.get(tile_id)
        {
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.shift {
                let ordered_nodes =
                    Self::tab_group_node_order_for_tile(tiles, tile_id).unwrap_or_else(|| vec![*node_key]);
                let target_index = ordered_nodes
                    .iter()
                    .position(|key| *key == *node_key)
                    .unwrap_or(0);
                let anchor_key = self.graph_app.tab_selection_anchor.unwrap_or(*node_key);
                let anchor_index = ordered_nodes
                    .iter()
                    .position(|key| *key == anchor_key)
                    .unwrap_or(target_index);
                if !modifiers.ctrl {
                    self.graph_app.selected_tab_nodes.clear();
                }
                if let Some(range) =
                    inclusive_index_range(anchor_index, target_index, ordered_nodes.len())
                {
                    self.graph_app
                        .add_tab_selection_keys(range.map(|idx| ordered_nodes[idx]));
                }
            } else if modifiers.ctrl {
                self.graph_app.toggle_tab_selection(*node_key);
            } else {
                self.graph_app.set_tab_selection_single(*node_key);
                self.pending_graph_intents.push(GraphIntent::SelectNode {
                    key: *node_key,
                    multi_select: false,
                });
            }
        }

        if tab_response.drag_stopped()
            && let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles.get(tile_id)
        {
            self.pending_tab_drag_stopped_nodes.insert(*node_key);
            if Self::should_detach_tab_on_drag_stop(ui, tab_rect) {
                self.graph_app.request_detach_node_to_split(*node_key);
            }
        }

        if ui.is_rect_visible(tab_rect) && !state.is_being_dragged {
            let mut bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, state);
            let mut stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, state);
            let tab_multi_selected = matches!(
                tiles.get(tile_id),
                Some(Tile::Pane(TileKind::WebView(node_key)))
                    if self.graph_app.selected_tab_nodes.contains(node_key)
            );
            if tab_multi_selected && !state.active {
                bg_color = bg_color.linear_multiply(1.08);
                stroke = Stroke::new(stroke.width.max(1.5), Color32::from_rgb(95, 170, 255));
            }
            ui.painter().rect(
                tab_rect.shrink(0.5),
                0.0,
                bg_color,
                stroke,
                egui::StrokeKind::Inside,
            );

            if state.active {
                ui.painter().hline(
                    tab_rect.x_range(),
                    tab_rect.bottom(),
                    Stroke::new(stroke.width + 1.0, bg_color),
                );
            }

            let mut text_rect = tab_rect.shrink(x_margin);
            if let Some(texture_id) = favicon_texture {
                let icon_rect = egui::Align2::LEFT_CENTER
                    .align_size_within_rect(vec2(icon_size, icon_size), text_rect);
                ui.painter().image(
                    texture_id,
                    icon_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                text_rect.min.x += icon_size + icon_spacing;
            }

            let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, state);
            let text_position = egui::Align2::LEFT_CENTER
                .align_size_within_rect(galley.size(), text_rect)
                .min;
            ui.painter().galley(text_position, galley, text_color);

            if state.closable {
                let close_btn_rect = egui::Align2::RIGHT_CENTER
                    .align_size_within_rect(close_btn_size, tab_rect.shrink(x_margin));

                let close_btn_id = ui.auto_id_with("tab_close_btn");
                let close_btn_response = ui
                    .interact(close_btn_rect, close_btn_id, Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::Default);

                let visuals = ui.style().interact(&close_btn_response);
                let rect = close_btn_rect
                    .shrink(self.close_button_inner_margin())
                    .expand(visuals.expansion);
                let stroke = visuals.fg_stroke;
                ui.painter()
                    .line_segment([rect.left_top(), rect.right_bottom()], stroke);
                ui.painter()
                    .line_segment([rect.right_top(), rect.left_bottom()], stroke);

                if close_btn_response.clicked() && self.on_tab_close(tiles, tile_id) {
                    tiles.remove(tile_id);
                }
            }
        }

        self.on_tab_button(tiles, tile_id, tab_response)
    }

    fn is_tab_closable(&self, tiles: &Tiles<TileKind>, tile_id: TileId) -> bool {
        match tiles.get(tile_id) {
            Some(Tile::Pane(TileKind::WebView(_))) => true,
            Some(Tile::Pane(TileKind::Graph)) => false,
            _ => false,
        }
    }

    fn on_tab_close(&mut self, tiles: &mut Tiles<TileKind>, tile_id: TileId) -> bool {
        if let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles.get(tile_id) {
            self.pending_closed_nodes.push(*node_key);
            self.graph_app.selected_tab_nodes.remove(node_key);
            if self.graph_app.tab_selection_anchor == Some(*node_key) {
                self.graph_app.tab_selection_anchor = None;
            }
        }
        true
    }
}
