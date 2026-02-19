/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Key, Modifiers, TopBottomPanel, Vec2, WidgetInfo, WidgetType};
use egui_tiles::Tree;
use euclid::default::Point2D;
use std::collections::HashSet;
use winit::window::Window;

use super::tile_grouping;
use super::toolbar_routing::{self, ToolbarNavAction, ToolbarOpenMode};
use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::tile_kind::TileKind;
use crate::graph::NodeKey;
use crate::running_app_state::{RunningAppState, UserInterfaceCommand};
use crate::search::{fuzzy_match_items, fuzzy_match_node_keys};
use crate::window::ServoShellWindow;

const WORKSPACE_PIN_NAME: &str = "workspace:pin:space";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OmnibarSearchMode {
    Mixed,
    NodesLocal,
    NodesAll,
    TabsLocal,
    TabsAll,
    EdgesLocal,
    EdgesAll,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OmnibarMatch {
    Node(NodeKey),
    NodeUrl(String),
    Edge { from: NodeKey, to: NodeKey },
}

#[derive(Clone)]
struct OmnibarSearchCandidate {
    text: String,
    target: OmnibarMatch,
}

impl AsRef<str> for OmnibarSearchCandidate {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

pub(crate) struct OmnibarSearchSession {
    mode: OmnibarSearchMode,
    pub(crate) query: String,
    pub(crate) matches: Vec<OmnibarMatch>,
    pub(crate) active_index: usize,
}

pub(crate) struct ToolbarUiArgs<'a> {
    pub ctx: &'a egui::Context,
    pub winit_window: &'a Window,
    pub state: &'a RunningAppState,
    pub graph_app: &'a mut GraphBrowserApp,
    pub window: &'a ServoShellWindow,
    pub tiles_tree: &'a Tree<TileKind>,
    pub focused_toolbar_node: Option<NodeKey>,
    pub has_webview_tiles: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub location: &'a mut String,
    pub location_dirty: &'a mut bool,
    pub location_submitted: &'a mut bool,
    pub focus_location_field_for_search: bool,
    pub show_data_dir_dialog: &'a mut bool,
    pub show_persistence_settings_dialog: &'a mut bool,
    pub show_clear_data_confirm: &'a mut bool,
    pub omnibar_search_session: &'a mut Option<OmnibarSearchSession>,
    pub frame_intents: &'a mut Vec<GraphIntent>,
}

pub(crate) struct ToolbarUiOutput {
    pub toggle_tile_view_requested: bool,
    pub open_selected_mode_after_submit: Option<ToolbarOpenMode>,
    pub toolbar_visible: bool,
}

fn toolbar_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(text)
        .frame(false)
        .min_size(Vec2 { x: 20.0, y: 20.0 })
}

fn workspace_pin_name_for_node(node: NodeKey, graph_app: &GraphBrowserApp) -> Option<String> {
    graph_app
        .graph
        .get_node(node)
        .map(|n| format!("workspace:pin:pane:{}", n.id))
}

pub(crate) fn render_toolbar_ui(args: ToolbarUiArgs<'_>) -> ToolbarUiOutput {
    let ToolbarUiArgs {
        ctx,
        winit_window,
        state,
        graph_app,
        window,
        tiles_tree,
        focused_toolbar_node,
        has_webview_tiles,
        can_go_back,
        can_go_forward,
        location,
        location_dirty,
        location_submitted,
        focus_location_field_for_search,
        show_data_dir_dialog,
        show_persistence_settings_dialog,
        show_clear_data_confirm,
        omnibar_search_session,
        frame_intents,
    } = args;

    if winit_window.fullscreen().is_some() {
        let fullscreen_url = focused_toolbar_node
            .and_then(|key| graph_app.graph.get_node(key).map(|node| node.url.clone()))
            .unwrap_or_else(|| "about:blank".to_string());
        let frame = egui::Frame::default()
            .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 25, 220))
            .inner_margin(4.0);
        TopBottomPanel::top("fullscreen_origin_strip")
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fullscreen");
                    ui.separator();
                    ui.label(fullscreen_url);
                    ui.separator();
                    ui.label("Press Esc to exit");
                });
            });
        return ToolbarUiOutput {
            toggle_tile_view_requested: false,
            open_selected_mode_after_submit: None,
            toolbar_visible: false,
        };
    }

    let mut toggle_tile_view_requested = false;
    let mut open_selected_mode_after_submit = None;
    let is_graph_view = !has_webview_tiles;
    let persisted_workspace_names: HashSet<String> = graph_app
        .list_workspace_layout_names()
        .into_iter()
        .collect();
    let focused_pane_pin_name =
        focused_toolbar_node.and_then(|node| workspace_pin_name_for_node(node, graph_app));

    let frame = egui::Frame::default()
        .fill(ctx.style().visuals.window_fill)
        .inner_margin(4.0);
    TopBottomPanel::top("toolbar").frame(frame).show(ctx, |ui| {
        ui.allocate_ui_with_layout(
            ui.available_size(),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let back_button = ui.add_enabled(can_go_back, toolbar_button("<"));
                back_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Back".into());
                    info
                });
                if back_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Back,
                    );
                }

                let forward_button = ui.add_enabled(can_go_forward, toolbar_button(">"));
                forward_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Forward".into());
                    info
                });
                if forward_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Forward,
                    );
                }

                let reload_button = ui.add(toolbar_button("R"));
                reload_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Reload".into());
                    info
                });
                if reload_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Reload,
                    );
                }
                ui.add_space(2.0);

                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let mut experimental_preferences_enabled =
                            state.experimental_preferences_enabled();
                        let prefs_toggle = ui
                            .toggle_value(&mut experimental_preferences_enabled, "Exp")
                            .on_hover_text("Enable experimental prefs");
                        prefs_toggle.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Enable experimental preferences".into());
                            info.selected = Some(experimental_preferences_enabled);
                            info
                        });
                        if prefs_toggle.clicked() {
                            state.set_experimental_preferences_enabled(
                                experimental_preferences_enabled,
                            );
                            *location_dirty = false;
                            window.queue_user_interface_command(UserInterfaceCommand::ReloadAll);
                        }

                        let (view_icon, view_tooltip) = if has_webview_tiles {
                            ("Graph", "Switch to Graph View")
                        } else {
                            ("Detail", "Switch to Detail View")
                        };
                        let view_toggle_button = ui
                            .add(toolbar_button(view_icon))
                            .on_hover_text(view_tooltip);
                        view_toggle_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Toggle View".into());
                            info
                        });
                        if view_toggle_button.clicked() {
                            toggle_tile_view_requested = true;
                        }

                        let data_dir_button = ui
                            .add(toolbar_button("Dir"))
                            .on_hover_text("Switch graph data directory");
                        data_dir_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Switch graph data directory".into());
                            info
                        });
                        if data_dir_button.clicked() {
                            *show_data_dir_dialog = true;
                        }

                        let persistence_settings_button = ui
                            .add(toolbar_button("Cfg"))
                            .on_hover_text("Persistence settings");
                        persistence_settings_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Persistence settings".into());
                            info
                        });
                        if persistence_settings_button.clicked() {
                            *show_persistence_settings_dialog = true;
                        }

                        let clear_data_button = ui
                            .add(toolbar_button("Clr"))
                            .on_hover_text("Clear graph and saved data");
                        clear_data_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Clear graph and saved data".into());
                            info
                        });
                        if clear_data_button.clicked() {
                            *show_clear_data_confirm = true;
                        }

                        let command_button = ui
                            .add(toolbar_button("Cmd"))
                            .on_hover_text("Open command palette (F2)");
                        if command_button.clicked() {
                            frame_intents.push(GraphIntent::ToggleCommandPalette);
                        }

                        let persistence_button = ui
                            .add(toolbar_button("Persist"))
                            .on_hover_text("Open persistence hub");
                        if persistence_button.clicked() {
                            frame_intents.push(GraphIntent::TogglePersistencePanel);
                        }

                        if has_webview_tiles {
                            if let Some(pane_pin_name) = focused_pane_pin_name.clone() {
                                let pane_is_pinned =
                                    persisted_workspace_names.contains(&pane_pin_name);
                                let pane_pin_label = if pane_is_pinned { "P-" } else { "P+" };
                                let pane_pin_button = ui.add(toolbar_button(pane_pin_label)).on_hover_text(
                                    if pane_is_pinned {
                                        "Unpin focused pane workspace snapshot"
                                    } else {
                                        "Pin focused pane workspace snapshot"
                                    },
                                );
                                if pane_pin_button.clicked() {
                                    if pane_is_pinned {
                                        if let Err(e) = graph_app.delete_workspace_layout(&pane_pin_name)
                                        {
                                            log::warn!(
                                                "Failed to unpin focused pane workspace '{pane_pin_name}': {e}"
                                            );
                                        }
                                    } else {
                                        graph_app.request_save_workspace_snapshot_named(
                                            pane_pin_name.clone(),
                                        );
                                    }
                                }

                                let pane_recall_button = ui
                                    .add_enabled(pane_is_pinned, toolbar_button("PR"))
                                    .on_hover_text("Recall focused pane pinned workspace");
                                if pane_recall_button.clicked() {
                                    graph_app.request_restore_workspace_snapshot_named(
                                        pane_pin_name.clone(),
                                    );
                                }
                            }

                            let space_is_pinned = persisted_workspace_names.contains(WORKSPACE_PIN_NAME);
                            let space_pin_label = if space_is_pinned { "W-" } else { "W+" };
                            let space_pin_button = ui.add(toolbar_button(space_pin_label)).on_hover_text(
                                if space_is_pinned {
                                    "Unpin current workspace snapshot"
                                } else {
                                    "Pin current workspace snapshot"
                                },
                            );
                            if space_pin_button.clicked() {
                                if space_is_pinned {
                                    if let Err(e) = graph_app.delete_workspace_layout(WORKSPACE_PIN_NAME)
                                    {
                                        log::warn!(
                                            "Failed to unpin workspace snapshot '{WORKSPACE_PIN_NAME}': {e}"
                                        );
                                    }
                                } else {
                                    graph_app.request_save_workspace_snapshot_named(
                                        WORKSPACE_PIN_NAME.to_string(),
                                    );
                                }
                            }

                            let space_recall_button = ui
                                .add_enabled(space_is_pinned, toolbar_button("WR"))
                                .on_hover_text("Recall pinned workspace snapshot");
                            if space_recall_button.clicked() {
                                graph_app.request_restore_workspace_snapshot_named(
                                    WORKSPACE_PIN_NAME.to_string(),
                                );
                            }
                        }

                        let location_id = egui::Id::new("location_input");
                        let location_field = ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::singleline(location)
                                .id(location_id)
                                .hint_text("Search or enter address"),
                        );

                        if location_field.changed() {
                            *location_dirty = true;
                        }
                        if focus_location_field_for_search
                            || ui.input(|i| {
                                if cfg!(target_os = "macos") {
                                    i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                } else {
                                    i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                        || i.clone().consume_key(Modifiers::ALT, Key::D)
                                }
                            })
                        {
                            location_field.request_focus();
                        }
                        if location_field.gained_focus()
                            && let Some(mut state) = TextEditState::load(ui.ctx(), location_id)
                        {
                            state.cursor.set_char_range(Some(CCursorRange::two(
                                CCursor::new(0),
                                CCursor::new(location.len()),
                            )));
                            state.store(ui.ctx(), location_id);
                        }

                        if let Some(query) = location.trim().strip_prefix('@') {
                            let (mode, query) = parse_omnibar_search_query(query);
                            if let Some(session) = omnibar_search_session.as_ref()
                                && session.mode == mode
                                && session.query == query
                                && !session.matches.is_empty()
                            {
                                let counter = format!(
                                    "{}/{}",
                                    session.active_index + 1,
                                    session.matches.len()
                                );
                                let pos = location_field.rect.right_top() + Vec2::new(-8.0, 4.0);
                                ui.painter().text(
                                    pos,
                                    egui::Align2::RIGHT_TOP,
                                    counter,
                                    egui::FontId::proportional(11.0),
                                    egui::Color32::GRAY,
                                );
                            }
                        }

                        let enter_while_focused =
                            location_field.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if enter_while_focused {
                            *location_submitted = true;
                        }
                        let should_submit_now = enter_while_focused
                            || *location_submitted
                            || (location_field.lost_focus()
                                && ui.input(|i| i.key_pressed(Key::Enter)));
                        if should_submit_now {
                            *location_submitted = false;
                            let mut handled_omnibar_search = false;
                            let trimmed_location = location.trim();
                            if let Some(query) = trimmed_location.strip_prefix('@') {
                                let (mode, query) = parse_omnibar_search_query(query);
                                if query.is_empty() {
                                    *omnibar_search_session = None;
                                    *location_dirty = false;
                                    handled_omnibar_search = true;
                                }

                                if !handled_omnibar_search {
                                    let reuse_existing =
                                        omnibar_search_session.as_ref().is_some_and(|session| {
                                            session.mode == mode
                                                && session.query == query
                                                && !session.matches.is_empty()
                                        });
                                    if reuse_existing {
                                        if let Some(session) = omnibar_search_session.as_mut() {
                                            session.active_index =
                                                (session.active_index + 1) % session.matches.len();
                                        }
                                    } else {
                                        let matches = omnibar_matches_for_query(
                                            graph_app,
                                            tiles_tree,
                                            mode,
                                            query,
                                            has_webview_tiles,
                                        );
                                        if matches.is_empty() {
                                            *omnibar_search_session = None;
                                        } else {
                                            *omnibar_search_session = Some(OmnibarSearchSession {
                                                mode,
                                                query: query.to_string(),
                                                matches,
                                                active_index: 0,
                                            });
                                        }
                                    }

                                    if let Some(session) = omnibar_search_session.as_ref()
                                        && !session.matches.is_empty()
                                        && let Some(active_match) =
                                            session.matches.get(session.active_index).cloned()
                                    {
                                        apply_omnibar_match(
                                            graph_app,
                                            active_match,
                                            has_webview_tiles,
                                            frame_intents,
                                            &mut open_selected_mode_after_submit,
                                        );
                                    }
                                    // Keep the @query text sticky while cycling in detail mode,
                                    // otherwise toolbar sync may immediately overwrite it with URL.
                                    *location_dirty = true;
                                    handled_omnibar_search = true;
                                }
                            }

                            if !handled_omnibar_search {
                                *omnibar_search_session = None;
                                let split_open_requested = ui.input(|i| i.modifiers.shift);
                                let submit_result = toolbar_routing::submit_address_bar_intents(
                                    graph_app,
                                    location,
                                    is_graph_view,
                                    focused_toolbar_node,
                                    split_open_requested,
                                    window,
                                    &state.servoshell_preferences.searchpage,
                                );
                                frame_intents.extend(submit_result.intents);
                                if submit_result.mark_clean {
                                    *location_dirty = false;
                                    open_selected_mode_after_submit = submit_result.open_mode;
                                }
                            }
                        }
                    },
                );
            },
        );
    });

    ToolbarUiOutput {
        toggle_tile_view_requested,
        open_selected_mode_after_submit,
        toolbar_visible: true,
    }
}

fn parse_omnibar_search_query(raw: &str) -> (OmnibarSearchMode, &str) {
    let trimmed = raw.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or_default();
    let tail = parts.next().unwrap_or_default().trim();
    if head == "T" {
        return (OmnibarSearchMode::TabsAll, tail);
    }
    if head.eq_ignore_ascii_case("t") || head.eq_ignore_ascii_case("tab") {
        return (OmnibarSearchMode::TabsLocal, tail);
    }
    if head == "N" {
        return (OmnibarSearchMode::NodesAll, tail);
    }
    if head.eq_ignore_ascii_case("n") || head.eq_ignore_ascii_case("node") {
        return (OmnibarSearchMode::NodesLocal, tail);
    }
    if head == "E" {
        return (OmnibarSearchMode::EdgesAll, tail);
    }
    if head.eq_ignore_ascii_case("e") || head.eq_ignore_ascii_case("edge") {
        return (OmnibarSearchMode::EdgesLocal, tail);
    }
    (OmnibarSearchMode::Mixed, trimmed)
}

fn tab_node_keys_in_tree(tiles_tree: &Tree<TileKind>) -> HashSet<NodeKey> {
    tile_grouping::webview_tab_group_memberships(tiles_tree)
        .keys()
        .copied()
        .collect()
}

fn tab_node_keys_in_workspace_layout_json(layout_json: &str) -> HashSet<NodeKey> {
    serde_json::from_str::<Tree<TileKind>>(layout_json)
        .ok()
        .map(|tree| {
            tile_grouping::webview_tab_group_memberships(&tree)
                .keys()
                .copied()
                .collect()
        })
        .unwrap_or_default()
}

fn edge_type_label(edge_type: crate::graph::EdgeType) -> &'static str {
    match edge_type {
        crate::graph::EdgeType::Hyperlink => "hyperlink",
        crate::graph::EdgeType::History => "history",
        crate::graph::EdgeType::UserGrouped => "user_grouped",
    }
}

fn graph_center_for_new_node(graph_app: &GraphBrowserApp) -> Point2D<f32> {
    let mut count = 0usize;
    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    for (_, node) in graph_app.graph.nodes() {
        sum_x += node.position.x;
        sum_y += node.position.y;
        count += 1;
    }
    if count == 0 {
        Point2D::new(0.0, 0.0)
    } else {
        Point2D::new(sum_x / count as f32, sum_y / count as f32)
    }
}

fn edge_candidates_for_graph(
    graph: &crate::graph::Graph,
    only_targets: Option<&HashSet<NodeKey>>,
) -> Vec<OmnibarSearchCandidate> {
    let mut out = Vec::new();
    for edge in graph.edges() {
        if let Some(filter) = only_targets
            && (!filter.contains(&edge.from) || !filter.contains(&edge.to))
        {
            continue;
        }
        let Some(from_node) = graph.get_node(edge.from) else {
            continue;
        };
        let Some(to_node) = graph.get_node(edge.to) else {
            continue;
        };
        out.push(OmnibarSearchCandidate {
            text: format!(
                "{} {} {} {} {}",
                edge_type_label(edge.edge_type),
                from_node.title,
                from_node.url,
                to_node.title,
                to_node.url
            ),
            target: OmnibarMatch::Edge {
                from: edge.from,
                to: edge.to,
            },
        });
    }
    out
}

fn node_candidates_for_graph(graph: &crate::graph::Graph) -> Vec<OmnibarSearchCandidate> {
    graph
        .nodes()
        .map(|(key, node)| OmnibarSearchCandidate {
            text: format!("{} {}", node.title, node.url),
            target: OmnibarMatch::Node(key),
        })
        .collect()
}

fn tab_candidates_for_keys(
    graph: &crate::graph::Graph,
    keys: &HashSet<NodeKey>,
) -> Vec<OmnibarSearchCandidate> {
    keys.iter()
        .filter_map(|key| {
            graph.get_node(*key).map(|node| OmnibarSearchCandidate {
                text: format!("{} {}", node.title, node.url),
                target: OmnibarMatch::Node(*key),
            })
        })
        .collect()
}

fn dedupe_matches_in_order(matches: Vec<OmnibarMatch>) -> Vec<OmnibarMatch> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for m in matches {
        if seen.insert(m.clone()) {
            out.push(m);
        }
    }
    out
}

fn ranked_matches(candidates: Vec<OmnibarSearchCandidate>, query: &str) -> Vec<OmnibarMatch> {
    dedupe_matches_in_order(
        fuzzy_match_items(candidates, query)
            .into_iter()
            .map(|candidate| candidate.target)
            .collect(),
    )
}

fn apply_omnibar_match(
    graph_app: &GraphBrowserApp,
    active_match: OmnibarMatch,
    has_webview_tiles: bool,
    frame_intents: &mut Vec<GraphIntent>,
    open_selected_mode_after_submit: &mut Option<ToolbarOpenMode>,
) {
    match active_match {
        OmnibarMatch::Node(key) => {
            frame_intents.push(GraphIntent::ClearHighlightedEdge);
            if has_webview_tiles {
                frame_intents.push(GraphIntent::OpenNodeWorkspaceRouted {
                    key,
                    prefer_workspace: None,
                });
            } else {
                frame_intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            }
        },
        OmnibarMatch::NodeUrl(url) => {
            frame_intents.push(GraphIntent::ClearHighlightedEdge);
            if let Some((key, _)) = graph_app.graph.get_node_by_url(&url) {
                if has_webview_tiles {
                    frame_intents.push(GraphIntent::OpenNodeWorkspaceRouted {
                        key,
                        prefer_workspace: None,
                    });
                } else {
                    frame_intents.push(GraphIntent::SelectNode {
                        key,
                        multi_select: false,
                    });
                }
            } else {
                frame_intents.push(GraphIntent::CreateNodeAtUrl {
                    url,
                    position: graph_center_for_new_node(graph_app),
                });
                if has_webview_tiles {
                    *open_selected_mode_after_submit = Some(ToolbarOpenMode::Tab);
                }
            }
        },
        OmnibarMatch::Edge { from, to } => {
            frame_intents.push(GraphIntent::SetHighlightedEdge { from, to });
            frame_intents.push(GraphIntent::SelectNode {
                key: from,
                multi_select: false,
            });
            frame_intents.push(GraphIntent::SelectNode {
                key: to,
                multi_select: true,
            });
        },
    }
}

fn omnibar_matches_for_query(
    graph_app: &GraphBrowserApp,
    tiles_tree: &Tree<TileKind>,
    mode: OmnibarSearchMode,
    query: &str,
    has_webview_tiles: bool,
) -> Vec<OmnibarMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let local_tab_nodes = tab_node_keys_in_tree(tiles_tree);
    let local_node_candidates = node_candidates_for_graph(&graph_app.graph);
    let local_edge_candidates = edge_candidates_for_graph(&graph_app.graph, None);

    let mut saved_tab_nodes = HashSet::new();
    for workspace_name in graph_app.list_workspace_layout_names() {
        if let Some(layout_json) = graph_app.load_workspace_layout_json(&workspace_name) {
            saved_tab_nodes.extend(tab_node_keys_in_workspace_layout_json(&layout_json));
        }
    }

    let mut all_graph_node_candidates = local_node_candidates.clone();
    let mut all_graph_edge_candidates = local_edge_candidates.clone();
    let mut node_urls_seen: HashSet<String> = graph_app
        .graph
        .nodes()
        .map(|(_, node)| node.url.clone())
        .collect();
    let mut mapped_edge_keys_seen: HashSet<(NodeKey, NodeKey)> =
        graph_app.graph.edges().map(|e| (e.from, e.to)).collect();

    if let Some(snapshot) = graph_app.peek_latest_graph_snapshot() {
        for (_, node) in snapshot.nodes() {
            if node_urls_seen.insert(node.url.clone()) {
                all_graph_node_candidates.push(OmnibarSearchCandidate {
                    text: format!("{} {}", node.title, node.url),
                    target: OmnibarMatch::NodeUrl(node.url.clone()),
                });
            }
        }
        for edge in snapshot.edges() {
            let Some(from_node) = snapshot.get_node(edge.from) else {
                continue;
            };
            let Some(to_node) = snapshot.get_node(edge.to) else {
                continue;
            };
            let current_from = graph_app
                .graph
                .get_node_by_url(&from_node.url)
                .map(|(k, _)| k);
            let current_to = graph_app
                .graph
                .get_node_by_url(&to_node.url)
                .map(|(k, _)| k);
            if let (Some(from_key), Some(to_key)) = (current_from, current_to)
                && mapped_edge_keys_seen.insert((from_key, to_key))
            {
                all_graph_edge_candidates.push(OmnibarSearchCandidate {
                    text: format!(
                        "{} {} {} {} {}",
                        edge_type_label(edge.edge_type),
                        from_node.title,
                        from_node.url,
                        to_node.title,
                        to_node.url
                    ),
                    target: OmnibarMatch::Edge {
                        from: from_key,
                        to: to_key,
                    },
                });
            }
        }
    }

    for name in graph_app.list_named_graph_snapshot_names() {
        if let Some(snapshot) = graph_app.peek_named_graph_snapshot(&name) {
            for (_, node) in snapshot.nodes() {
                if node_urls_seen.insert(node.url.clone()) {
                    all_graph_node_candidates.push(OmnibarSearchCandidate {
                        text: format!("{} {}", node.title, node.url),
                        target: OmnibarMatch::NodeUrl(node.url.clone()),
                    });
                }
            }
            for edge in snapshot.edges() {
                let Some(from_node) = snapshot.get_node(edge.from) else {
                    continue;
                };
                let Some(to_node) = snapshot.get_node(edge.to) else {
                    continue;
                };
                let current_from = graph_app
                    .graph
                    .get_node_by_url(&from_node.url)
                    .map(|(k, _)| k);
                let current_to = graph_app
                    .graph
                    .get_node_by_url(&to_node.url)
                    .map(|(k, _)| k);
                if let (Some(from_key), Some(to_key)) = (current_from, current_to)
                    && mapped_edge_keys_seen.insert((from_key, to_key))
                {
                    all_graph_edge_candidates.push(OmnibarSearchCandidate {
                        text: format!(
                            "{} {} {} {} {}",
                            edge_type_label(edge.edge_type),
                            from_node.title,
                            from_node.url,
                            to_node.title,
                            to_node.url
                        ),
                        target: OmnibarMatch::Edge {
                            from: from_key,
                            to: to_key,
                        },
                    });
                }
            }
        }
    }

    let local_tab_candidates = tab_candidates_for_keys(&graph_app.graph, &local_tab_nodes);
    let all_tab_keys: HashSet<NodeKey> = local_tab_nodes
        .iter()
        .copied()
        .chain(saved_tab_nodes.iter().copied())
        .collect();
    let all_tab_candidates = tab_candidates_for_keys(&graph_app.graph, &all_tab_keys);

    match mode {
        OmnibarSearchMode::NodesLocal => ranked_matches(local_node_candidates, query),
        OmnibarSearchMode::NodesAll => ranked_matches(all_graph_node_candidates, query),
        OmnibarSearchMode::TabsLocal => ranked_matches(local_tab_candidates, query),
        OmnibarSearchMode::TabsAll => ranked_matches(all_tab_candidates, query),
        OmnibarSearchMode::EdgesLocal => ranked_matches(local_edge_candidates, query),
        OmnibarSearchMode::EdgesAll => ranked_matches(all_graph_edge_candidates, query),
        OmnibarSearchMode::Mixed => {
            let node_matches = fuzzy_match_node_keys(&graph_app.graph, query);
            if node_matches.is_empty() {
                return Vec::new();
            }
            let local_tab_set = tab_node_keys_in_tree(tiles_tree);
            let tab_matches: Vec<OmnibarMatch> = node_matches
                .iter()
                .copied()
                .filter(|key| local_tab_set.contains(key))
                .map(OmnibarMatch::Node)
                .collect();
            if !has_webview_tiles {
                return node_matches.into_iter().map(OmnibarMatch::Node).collect();
            }
            let mut out = tab_matches;
            out.extend(
                node_matches
                    .into_iter()
                    .filter(|key| !local_tab_set.contains(key))
                    .map(OmnibarMatch::Node),
            );
            out
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::GraphBrowserApp;
    use crate::desktop::tile_kind::TileKind;
    use crate::graph::EdgeType;
    use egui_tiles::Tree;
    use euclid::default::Point2D;
    use tempfile::TempDir;

    #[test]
    fn test_parse_omnibar_search_query_modes() {
        assert_eq!(
            parse_omnibar_search_query("t rust"),
            (OmnibarSearchMode::TabsLocal, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("n rust"),
            (OmnibarSearchMode::NodesLocal, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("N rust"),
            (OmnibarSearchMode::NodesAll, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("T rust"),
            (OmnibarSearchMode::TabsAll, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("e rust"),
            (OmnibarSearchMode::EdgesLocal, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("E rust"),
            (OmnibarSearchMode::EdgesAll, "rust")
        );
        assert_eq!(
            parse_omnibar_search_query("rust"),
            (OmnibarSearchMode::Mixed, "rust")
        );
    }

    #[test]
    fn test_omnibar_tabs_mode_limits_results_to_tab_nodes() {
        let mut app = GraphBrowserApp::new_for_testing();
        let tab_key = app.add_node_and_sync("https://alpha-tab.example".into(), Point2D::zero());
        let non_tab_key =
            app.add_node_and_sync("https://alpha-node.example".into(), Point2D::new(20.0, 0.0));

        let mut tiles = egui_tiles::Tiles::default();
        let tab_tile = tiles.insert_pane(TileKind::WebView(tab_key));
        let tabs = tiles.insert_tab_tile(vec![tab_tile]);
        let tree = Tree::new("tabs_mode_test", tabs, tiles);

        let matches =
            omnibar_matches_for_query(&app, &tree, OmnibarSearchMode::TabsLocal, "alpha", true);
        assert_eq!(matches, vec![OmnibarMatch::Node(tab_key)]);
        assert!(!matches.contains(&OmnibarMatch::Node(non_tab_key)));
    }

    #[test]
    fn test_omnibar_mixed_mode_prioritizes_tab_nodes_in_detail_mode() {
        let mut app = GraphBrowserApp::new_for_testing();
        let tab_key = app.add_node_and_sync("https://beta-tab.example".into(), Point2D::zero());
        let node_key =
            app.add_node_and_sync("https://beta-node.example".into(), Point2D::new(20.0, 0.0));

        let mut tiles = egui_tiles::Tiles::default();
        let tab_tile = tiles.insert_pane(TileKind::WebView(tab_key));
        let tabs = tiles.insert_tab_tile(vec![tab_tile]);
        let tree = Tree::new("mixed_mode_test", tabs, tiles);

        let matches =
            omnibar_matches_for_query(&app, &tree, OmnibarSearchMode::Mixed, "beta", true);
        assert!(!matches.is_empty());
        assert_eq!(matches.first().cloned(), Some(OmnibarMatch::Node(tab_key)));
        assert!(matches.contains(&OmnibarMatch::Node(node_key)));
    }

    #[test]
    fn test_omnibar_nodes_all_includes_saved_graph_nodes() {
        let temp = TempDir::new().expect("temp dir");
        let mut app = GraphBrowserApp::new_from_dir(temp.path().to_path_buf());
        let _saved_key =
            app.add_node_and_sync("https://saved-node.example".into(), Point2D::zero());
        app.save_named_graph_snapshot("saved-graph")
            .expect("save named graph snapshot");

        app.clear_graph();
        let _active_key = app.add_node_and_sync(
            "https://active-node.example".into(),
            Point2D::new(10.0, 10.0),
        );

        let mut tiles = egui_tiles::Tiles::default();
        let root = tiles.insert_pane(TileKind::Graph);
        let tree = Tree::new("nodes_all_test", root, tiles);

        let matches = omnibar_matches_for_query(
            &app,
            &tree,
            OmnibarSearchMode::NodesAll,
            "saved-node",
            false,
        );
        assert!(
            matches.contains(&OmnibarMatch::NodeUrl("https://saved-node.example".into())),
            "expected @N results to include saved graph node by URL"
        );
    }

    #[test]
    fn test_omnibar_tabs_all_includes_saved_workspace_tabs() {
        let temp = TempDir::new().expect("temp dir");
        let mut app = GraphBrowserApp::new_from_dir(temp.path().to_path_buf());
        let tab_key = app.add_node_and_sync("https://saved-tab.example".into(), Point2D::zero());

        let mut workspace_tiles = egui_tiles::Tiles::default();
        let tab_leaf = workspace_tiles.insert_pane(TileKind::WebView(tab_key));
        let tabs_root = workspace_tiles.insert_tab_tile(vec![tab_leaf]);
        let workspace_tree = Tree::new("saved_workspace", tabs_root, workspace_tiles);
        let layout_json = serde_json::to_string(&workspace_tree).expect("serialize workspace");
        app.save_workspace_layout_json("workspace:saved-tabs", &layout_json);

        let mut current_tiles = egui_tiles::Tiles::default();
        let current_root = current_tiles.insert_pane(TileKind::Graph);
        let current_tree = Tree::new("current_tree", current_root, current_tiles);

        let matches = omnibar_matches_for_query(
            &app,
            &current_tree,
            OmnibarSearchMode::TabsAll,
            "saved-tab",
            true,
        );
        assert_eq!(matches, vec![OmnibarMatch::Node(tab_key)]);
    }

    #[test]
    fn test_omnibar_edges_all_includes_saved_graph_edges_when_nodes_map_by_url() {
        let temp = TempDir::new().expect("temp dir");
        let mut app = GraphBrowserApp::new_from_dir(temp.path().to_path_buf());
        let from = app.add_node_and_sync("https://edge-a.example".into(), Point2D::zero());
        let to = app.add_node_and_sync("https://edge-b.example".into(), Point2D::new(20.0, 0.0));
        let _ = app.add_edge_and_sync(from, to, EdgeType::UserGrouped);
        app.save_named_graph_snapshot("saved-edge-graph")
            .expect("save named graph snapshot");
        let _ = app.remove_edges_and_log(from, to, EdgeType::UserGrouped);

        let mut tiles = egui_tiles::Tiles::default();
        let root = tiles.insert_pane(TileKind::Graph);
        let tree = Tree::new("edges_all_test", root, tiles);

        let matches =
            omnibar_matches_for_query(&app, &tree, OmnibarSearchMode::EdgesAll, "edge-a", false);
        assert_eq!(matches, vec![OmnibarMatch::Edge { from, to }]);
    }

    #[test]
    fn test_apply_omnibar_edge_match_sets_highlight_and_pair_selection() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app.add_node_and_sync("https://from.example".into(), Point2D::zero());
        let to = app.add_node_and_sync("https://to.example".into(), Point2D::new(20.0, 0.0));
        let mut intents = Vec::new();
        let mut open_mode = None;

        apply_omnibar_match(
            &app,
            OmnibarMatch::Edge { from, to },
            false,
            &mut intents,
            &mut open_mode,
        );
        app.apply_intents(intents);

        assert_eq!(app.highlighted_graph_edge, Some((from, to)));
        assert!(app.selected_nodes.contains(&from));
        assert!(app.selected_nodes.contains(&to));
    }

    #[test]
    fn test_apply_omnibar_node_match_routes_workspace_open_in_detail_mode() {
        let app = GraphBrowserApp::new_for_testing();
        let key = NodeKey::new(7);
        let mut intents = Vec::new();
        let mut open_mode = None;

        apply_omnibar_match(
            &app,
            OmnibarMatch::Node(key),
            true,
            &mut intents,
            &mut open_mode,
        );

        assert!(intents.iter().any(|intent| {
            matches!(
                intent,
                GraphIntent::OpenNodeWorkspaceRouted {
                    key: routed_key,
                    prefer_workspace: None
                } if *routed_key == key
            )
        }));
        assert!(!intents.iter().any(|intent| {
            matches!(
                intent,
                GraphIntent::SelectNode {
                    key: selected_key,
                    multi_select: false
                } if *selected_key == key
            )
        }));
        assert!(open_mode.is_none());
    }

    #[test]
    fn test_apply_omnibar_node_url_existing_routes_workspace_open_in_detail_mode() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.add_node_and_sync("https://node-url.example".into(), Point2D::zero());
        let mut intents = Vec::new();
        let mut open_mode = None;

        apply_omnibar_match(
            &app,
            OmnibarMatch::NodeUrl("https://node-url.example".into()),
            true,
            &mut intents,
            &mut open_mode,
        );

        assert!(intents.iter().any(|intent| {
            matches!(
                intent,
                GraphIntent::OpenNodeWorkspaceRouted {
                    key: routed_key,
                    prefer_workspace: None
                } if *routed_key == key
            )
        }));
        assert!(open_mode.is_none());
    }

    #[test]
    fn test_apply_omnibar_node_url_new_keeps_open_selected_mode_for_new_node() {
        let app = GraphBrowserApp::new_for_testing();
        let mut intents = Vec::new();
        let mut open_mode = None;

        apply_omnibar_match(
            &app,
            OmnibarMatch::NodeUrl("https://new-node-url.example".into()),
            true,
            &mut intents,
            &mut open_mode,
        );

        assert!(intents.iter().any(|intent| {
            matches!(
                intent,
                GraphIntent::CreateNodeAtUrl { url, .. } if url == "https://new-node-url.example"
            )
        }));
        assert!(matches!(open_mode, Some(ToolbarOpenMode::Tab)));
    }
}
