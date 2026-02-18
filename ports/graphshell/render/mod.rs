/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Graph rendering module using egui_graphs.
//!
//! Delegates graph visualization and interaction to the egui_graphs crate,
//! which provides built-in navigation (zoom/pan), node dragging, and selection.

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::egui_adapter::{EguiGraphState, GraphNodeShape};
use crate::graph::{NodeKey, NodeLifecycle};
use egui::{Color32, Ui, Vec2, Window};
use egui_graphs::events::Event;
use egui_graphs::{
    DefaultEdgeShape, FruchtermanReingold, FruchtermanReingoldState, GraphView,
    LayoutForceDirected, MetadataFrame, SettingsInteraction, SettingsNavigation, SettingsStyle,
    get_layout_state, set_layout_state,
};
use euclid::default::Point2D;
use petgraph::stable_graph::NodeIndex;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Graph interaction action (resolved from egui_graphs events).
///
/// Decouples event conversion (needs `egui_state` for NodeIndex→NodeKey
/// lookups) from action application (pure state mutation), making
/// graph interactions testable without an egui rendering context.
pub enum GraphAction {
    FocusNode(NodeKey),
    FocusNodeSplit(NodeKey),
    DragStart,
    DragEnd(NodeKey, Point2D<f32>),
    MoveNode(NodeKey, Point2D<f32>),
    SelectNode(NodeKey),
    Zoom(f32),
}

/// Render graph info and controls hint overlay text into the current UI.
pub fn render_graph_info_in_ui(ui: &mut Ui, app: &GraphBrowserApp) {
    draw_graph_info(ui, app);
}

/// Render graph content and return resolved interaction actions.
///
/// This lets callers customize how specific actions are handled
/// (e.g. routing double-click to tile opening instead of detail view).
pub fn render_graph_in_ui_collect_actions(
    ui: &mut Ui,
    app: &mut GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    search_filter_mode: bool,
    search_query_active: bool,
) -> Vec<GraphAction> {
    let filtered_graph = if search_filter_mode && search_query_active {
        Some(filtered_graph_for_search(app, search_matches))
    } else {
        None
    };
    let graph_for_render = filtered_graph.as_ref().unwrap_or(&app.graph);

    // Build or reuse egui_graphs state (rebuild always when filtering is active).
    if app.egui_state.is_none() || app.egui_state_dirty || filtered_graph.is_some() {
        app.egui_state = Some(EguiGraphState::from_graph(
            graph_for_render,
            &app.selected_nodes,
        ));
        app.egui_state_dirty = false;
    }

    apply_search_node_visuals(
        app,
        search_matches,
        active_search_match,
        search_query_active,
    );

    // Event collection buffer
    let events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));

    // Navigation: use egui_graphs built-in zoom/pan
    let nav = SettingsNavigation::new()
        .with_fit_to_screen_enabled(app.fit_to_screen_requested)
        .with_zoom_and_pan_enabled(true)
        .with_zoom_speed(0.15);

    // Interaction: dragging, selection, clicking
    let interaction = SettingsInteraction::new()
        .with_dragging_enabled(true)
        .with_node_selection_enabled(true)
        .with_node_clicking_enabled(true);

    // Style: always show labels
    let style = SettingsStyle::new().with_labels_always(true);

    // Keep egui_graphs layout cache aligned with app-owned FR state.
    set_layout_state::<FruchtermanReingoldState>(ui, app.physics.clone(), None);

    // Render the graph (nested scope for mutable borrow)
    {
        let state = app
            .egui_state
            .as_mut()
            .expect("egui_state should be initialized");

        ui.add(
            &mut GraphView::<
                _,
                _,
                _,
                _,
                GraphNodeShape,
                DefaultEdgeShape,
                FruchtermanReingoldState,
                LayoutForceDirected<FruchtermanReingold>,
            >::new(&mut state.graph)
            .with_navigations(&nav)
            .with_interactions(&interaction)
            .with_styles(&style)
            .with_event_sink(&events),
        );
    } // Drop mutable borrow of app.egui_state here

    // Pull latest FR state from egui_graphs after this frame's layout step.
    app.physics = get_layout_state::<FruchtermanReingoldState>(ui, None);

    // Reset fit_to_screen flag (one-shot behavior for 'C' key)
    app.fit_to_screen_requested = false;

    // Post-frame zoom clamp: enforce min/max bounds on egui_graphs zoom
    clamp_zoom(ui.ctx(), app);

    let split_open_modifier = ui.input(|i| i.modifiers.shift);
    collect_graph_actions(app, &events, split_open_modifier)
}

fn filtered_graph_for_search(
    app: &GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
) -> crate::graph::Graph {
    let mut filtered = app.graph.clone();
    let to_remove: Vec<NodeKey> = filtered
        .nodes()
        .map(|(key, _)| key)
        .filter(|key| !search_matches.contains(key))
        .collect();
    for key in to_remove {
        filtered.remove_node(key);
    }
    filtered
}

fn lifecycle_color(lifecycle: NodeLifecycle) -> Color32 {
    match lifecycle {
        NodeLifecycle::Active => Color32::from_rgb(100, 200, 255),
        NodeLifecycle::Cold => Color32::from_rgb(140, 140, 165),
    }
}

fn apply_search_node_visuals(
    app: &mut GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    search_query_active: bool,
) {
    let colors: Vec<(NodeKey, Color32)> = app
        .graph
        .nodes()
        .map(|(key, node)| {
            let mut color = lifecycle_color(node.lifecycle);
            if app.selected_nodes.contains(&key) {
                color = Color32::from_rgb(255, 200, 100);
            }
            if search_query_active && search_matches.contains(&key) {
                color = if active_search_match == Some(key) {
                    Color32::from_rgb(140, 255, 140)
                } else {
                    Color32::from_rgb(95, 220, 130)
                };
            }
            (key, color)
        })
        .collect();

    let Some(state) = app.egui_state.as_mut() else {
        return;
    };
    for (key, color) in colors {
        if let Some(node) = state.graph.node_mut(key) {
            node.set_color(color);
        }
    }
}

/// Clamp the egui_graphs zoom to the camera's min/max bounds.
/// Reads MetadataFrame from egui's persisted data, clamps zoom, writes back if changed.
fn clamp_zoom(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    let meta_id = egui::Id::new("egui_graphs_metadata_");
    ctx.data_mut(|data| {
        if let Some(mut meta) = data.get_persisted::<MetadataFrame>(meta_id) {
            let clamped = app.camera.clamp(meta.zoom);
            app.camera.current_zoom = clamped;
            if (meta.zoom - clamped).abs() > f32::EPSILON {
                meta.zoom = clamped;
                data.insert_persisted(meta_id, meta);
            }
        }
    });
}

/// Convert egui_graphs events to resolved GraphActions and apply them.
fn collect_graph_actions(
    app: &GraphBrowserApp,
    events: &Rc<RefCell<Vec<Event>>>,
    split_open_modifier: bool,
) -> Vec<GraphAction> {
    let mut actions = Vec::new();

    for event in events.borrow_mut().drain(..) {
        match event {
            Event::NodeDoubleClick(p) => {
                if let Some(state) = app.egui_state.as_ref() {
                    let idx = NodeIndex::new(p.id);
                    if let Some(key) = state.get_key(idx) {
                        if split_open_modifier {
                            actions.push(GraphAction::FocusNodeSplit(key));
                        } else {
                            actions.push(GraphAction::FocusNode(key));
                        }
                    }
                }
            },
            Event::NodeDragStart(_) => {
                actions.push(GraphAction::DragStart);
            },
            Event::NodeDragEnd(p) => {
                // Resolve final position from egui_state
                let idx = NodeIndex::new(p.id);
                if let Some(state) = app.egui_state.as_ref() {
                    if let Some(key) = state.get_key(idx) {
                        let pos = state
                            .graph
                            .node(idx)
                            .map(|n| Point2D::new(n.location().x, n.location().y))
                            .unwrap_or_default();
                        actions.push(GraphAction::DragEnd(key, pos));
                    }
                }
            },
            Event::NodeMove(p) => {
                let idx = NodeIndex::new(p.id);
                if let Some(state) = app.egui_state.as_ref() {
                    if let Some(key) = state.get_key(idx) {
                        actions.push(GraphAction::MoveNode(
                            key,
                            Point2D::new(p.new_pos[0], p.new_pos[1]),
                        ));
                    }
                }
            },
            Event::NodeSelect(p) => {
                if let Some(state) = app.egui_state.as_ref() {
                    let idx = NodeIndex::new(p.id);
                    if let Some(key) = state.get_key(idx) {
                        actions.push(GraphAction::SelectNode(key));
                    }
                }
            },
            Event::NodeDeselect(_) => {
                // Selection clearing handled by the next SelectNode action
            },
            Event::Zoom(p) => {
                actions.push(GraphAction::Zoom(p.new_zoom));
            },
            _ => {},
        }
    }

    actions
}

/// Convert resolved graph actions to graph intents without applying them.
pub fn intents_from_graph_actions(actions: Vec<GraphAction>) -> Vec<GraphIntent> {
    let mut intents = Vec::with_capacity(actions.len());
    for action in actions {
        match action {
            GraphAction::FocusNode(key) => {
                intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            },
            GraphAction::FocusNodeSplit(key) => {
                intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            },
            GraphAction::DragStart => {
                intents.push(GraphIntent::SetInteracting { interacting: true });
            },
            GraphAction::DragEnd(key, pos) => {
                intents.push(GraphIntent::SetInteracting { interacting: false });
                intents.push(GraphIntent::SetNodePosition { key, position: pos });
            },
            GraphAction::MoveNode(key, pos) => {
                intents.push(GraphIntent::SetNodePosition { key, position: pos });
            },
            GraphAction::SelectNode(key) => {
                intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            },
            GraphAction::Zoom(new_zoom) => {
                intents.push(GraphIntent::SetZoom { zoom: new_zoom });
            },
        }
    }
    intents
}

/// Sync node positions from egui_graphs layout state back into app graph state.
///
/// Pinned nodes keep their app-authored positions; their visual positions are
/// restored after layout so FR simulation does not move them.
pub(crate) fn sync_graph_positions_from_layout(app: &mut GraphBrowserApp) {
    let Some(state) = app.egui_state.as_ref() else {
        return;
    };

    let layout_positions: Vec<(NodeKey, Point2D<f32>)> = app
        .graph
        .nodes()
        .filter_map(|(key, _)| {
            state
                .graph
                .node(key)
                .map(|n| (key, Point2D::new(n.location().x, n.location().y)))
        })
        .collect();

    let mut pinned_positions = Vec::new();
    for (key, pos) in layout_positions {
        if let Some(node_mut) = app.graph.get_node_mut(key) {
            if node_mut.is_pinned {
                pinned_positions.push((key, node_mut.position));
            } else {
                node_mut.position = pos;
            }
        }
    }

    if let Some(state_mut) = app.egui_state.as_mut() {
        for (key, pos) in pinned_positions {
            if let Some(egui_node) = state_mut.graph.node_mut(key) {
                egui_node.set_location(egui::Pos2::new(pos.x, pos.y));
            }
        }
    }
}

/// Draw graph information overlay
fn draw_graph_info(ui: &mut egui::Ui, app: &GraphBrowserApp) {
    let info_text = format!(
        "Nodes: {} | Edges: {} | Physics: {} | Zoom: {:.1}x",
        app.graph.node_count(),
        app.graph.edge_count(),
        if app.physics.is_running {
            "Running"
        } else {
            "Paused"
        },
        app.camera.current_zoom
    );

    ui.painter().text(
        ui.available_rect_before_wrap().left_top() + Vec2::new(10.0, 10.0),
        egui::Align2::LEFT_TOP,
        info_text,
        egui::FontId::monospace(12.0),
        Color32::from_rgb(200, 200, 200),
    );

    // Draw controls hint
    let controls_text = "Shortcuts: Double-click Open | Shift+Double-click Split Open | N New Node | Del Remove | T Physics | C Fit | Ctrl+F Search | Home/Esc Toggle View | F1/? Help";
    ui.painter().text(
        ui.available_rect_before_wrap().left_bottom() + Vec2::new(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        controls_text,
        egui::FontId::proportional(10.0),
        Color32::from_rgb(150, 150, 150),
    );
}

/// Render physics configuration panel
pub fn render_physics_panel(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    if !app.show_physics_panel {
        return;
    }

    Window::new("Physics Configuration")
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.heading("Force Parameters");

            let mut config = app.physics.clone();
            let mut config_changed = false;

            ui.add_space(8.0);

            ui.label("Repulsion (c_repulse):");
            if ui
                .add(egui::Slider::new(&mut config.c_repulse, 0.0..=10.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Attraction (c_attract):");
            if ui
                .add(egui::Slider::new(&mut config.c_attract, 0.0..=10.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Ideal Distance Scale (k_scale):");
            if ui
                .add(egui::Slider::new(&mut config.k_scale, 0.1..=5.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Max Step:");
            if ui
                .add(egui::Slider::new(&mut config.max_step, 0.1..=100.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.heading("Damping & Convergence");
            ui.add_space(8.0);

            ui.label("Damping:");
            if ui
                .add(egui::Slider::new(&mut config.damping, 0.01..=1.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Time Step (dt):");
            if ui
                .add(egui::Slider::new(&mut config.dt, 0.001..=1.0).logarithmic(true))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Epsilon:");
            if ui
                .add(egui::Slider::new(&mut config.epsilon, 1e-6..=0.1).logarithmic(true))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Reset button
            ui.horizontal(|ui| {
                if ui.button("Reset to Defaults").clicked() {
                    let running = config.is_running;
                    config = GraphBrowserApp::default_physics_state();
                    config.is_running = running;
                    config_changed = true;
                }

                ui.label(if app.physics.is_running {
                    "Status: Running"
                } else {
                    "Status: Paused"
                });
            });

            if let Some(last_avg) = app.physics.last_avg_displacement {
                ui.label(format!("Last avg displacement: {:.4}", last_avg));
            }
            ui.label(format!("Step count: {}", app.physics.step_count));

            // Apply config changes
            if config_changed {
                app.update_physics_config(config);
            }
        });
}

/// Render keyboard shortcut help panel
pub fn render_help_panel(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    if !app.show_help_panel {
        return;
    }

    let mut open = app.show_help_panel;
    Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .default_width(350.0)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("shortcut_grid")
                .num_columns(2)
                .spacing([20.0, 6.0])
                .show(ui, |ui| {
                    let shortcuts = [
                        ("Home / Esc", "Toggle Graph / Detail view"),
                        ("N", "Create new node"),
                        ("Delete", "Remove selected nodes"),
                        ("Ctrl+Shift+Delete", "Clear entire graph"),
                        ("T", "Toggle physics simulation"),
                        ("C", "Fit graph to screen"),
                        ("P", "Physics settings panel"),
                        ("Ctrl+F", "Show graph search"),
                        ("Search Up/Down", "Cycle graph matches"),
                        ("Search Enter", "Select active search match"),
                        ("F1 / ?", "This help panel"),
                        ("Ctrl+L / Alt+D", "Focus address bar"),
                        ("Double-click node", "Open node in detail view"),
                        ("Shift + Double-click node", "Open node in split pane"),
                        ("Click + drag", "Move a node"),
                        ("Scroll wheel", "Zoom in / out"),
                    ];

                    for (key, desc) in shortcuts {
                        ui.strong(key);
                        ui.label(desc);
                        ui.end_row();
                    }
                });
        });
    app.show_help_panel = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> GraphBrowserApp {
        GraphBrowserApp::new_for_testing()
    }

    #[test]
    fn test_focus_node_action() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents = intents_from_graph_actions(vec![GraphAction::FocusNode(key)]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&key));
    }

    #[test]
    fn test_drag_start_sets_interacting() {
        let mut app = test_app();
        assert!(!app.is_interacting);

        let intents = intents_from_graph_actions(vec![GraphAction::DragStart]);
        app.apply_intents(intents);

        assert!(app.is_interacting);
    }

    #[test]
    fn test_drag_end_clears_interacting_and_updates_position() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));
        app.set_interacting(true);

        let intents = intents_from_graph_actions(vec![GraphAction::DragEnd(
            key,
            Point2D::new(150.0, 250.0),
        )]);
        app.apply_intents(intents);

        assert!(!app.is_interacting);
        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.position, Point2D::new(150.0, 250.0));
    }

    #[test]
    fn test_move_node_updates_position() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents =
            intents_from_graph_actions(vec![GraphAction::MoveNode(key, Point2D::new(42.0, 84.0))]);
        app.apply_intents(intents);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.position, Point2D::new(42.0, 84.0));
    }

    #[test]
    fn test_select_node_action() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents = intents_from_graph_actions(vec![GraphAction::SelectNode(key)]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&key));
    }

    #[test]
    fn test_zoom_action_clamps() {
        let mut app = test_app();

        let intents = intents_from_graph_actions(vec![GraphAction::Zoom(0.01)]);
        app.apply_intents(intents);

        // Should be clamped to min zoom
        assert!(app.camera.current_zoom >= app.camera.zoom_min);
    }

    #[test]
    fn test_multiple_actions_sequence() {
        let mut app = test_app();
        let k1 = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let k2 = app.add_node_and_sync("b".into(), Point2D::new(100.0, 100.0));

        let intents = intents_from_graph_actions(vec![
            GraphAction::SelectNode(k1),
            GraphAction::MoveNode(k2, Point2D::new(200.0, 300.0)),
            GraphAction::Zoom(1.5),
        ]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&k1));
        assert_eq!(
            app.graph.get_node(k2).unwrap().position,
            Point2D::new(200.0, 300.0)
        );
        assert!((app.camera.current_zoom - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_empty_actions_is_noop() {
        let mut app = test_app();
        let key = app.add_node_and_sync("a".into(), Point2D::new(50.0, 60.0));
        let pos_before = app.graph.get_node(key).unwrap().position;

        let intents = intents_from_graph_actions(vec![]);
        app.apply_intents(intents);

        assert_eq!(app.graph.get_node(key).unwrap().position, pos_before);
    }
}
